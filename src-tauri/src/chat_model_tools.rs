//! Outils MCP exposes uniquement aux tours de chat normaux du serveur web.
//!
//! Chaque tour recoit un jeton de capacite aleatoire qui lie les actions du
//! modele au compte, au dossier et au chat source. Le modele ne peut donc ni
//! choisir un autre compte, ni rattacher l'agent a une autre conversation. Les
//! politiques partagees peuvent ignorer la cle du chat, mais jamais les bornes
//! de compte et de projet portees par cette capacite.

use crate::{
    autonomous::{
        AutonomousAgentSnapshot, AutonomousAgentStatus, AutonomousTriggerKind,
        CreateAutonomousAgentRequest, UpdateAutonomousAgentRequest,
    },
    chat::{ChatAppConnector, ChatTurnMode},
    metrics,
    terminal_goal::{TerminalGoalSnapshot, TerminalGoalStatus},
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

pub const AUTONOMOUS_AGENT_TOOL_NAME: &str = "create_autonomous_agent";
pub const CREATE_GOAL_TOOL_NAME: &str = "create_goal";
pub const GET_GOAL_TOOL_NAME: &str = "get_goal";
pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";
pub const UPDATE_AUTONOMOUS_AGENT_TOOL_NAME: &str = "update_autonomous_agent";
pub const PAUSE_AUTONOMOUS_AGENT_TOOL_NAME: &str = "pause_autonomous_agent";
pub const APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME: &str = "apply_autonomous_agent_policy";
pub const ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME: &str = "activate_supervisor_general_report";
pub const CREATE_CHAT_TOOL_NAME: &str = "create_chat";
pub const LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME: &str = "list_private_message_users";
pub const LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME: &str = "list_private_message_campaigns";
pub const CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME: &str = "create_private_message_campaign";
pub const CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME: &str = "control_private_message_campaign";
pub const LIST_CONTROL_DEVICES_TOOL_NAME: &str = "list_control_devices";
pub const CONTROL_DEVICE_TOOL_NAME: &str = "control_device";
pub const GET_CONTROL_DEVICE_ACTION_TOOL_NAME: &str = "get_control_device_action";
pub const LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME: &str = "list_tiktok_dm_campaigns";
pub const LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME: &str = "list_tiktok_sender_accounts";
pub const MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME: &str = "manage_tiktok_sender_login";
pub const SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME: &str = "select_tiktok_sender_account";
pub const PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME: &str = "prepare_tiktok_dm_campaign";
pub const SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME: &str = "send_tiktok_dm_campaign";
pub const LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME: &str = "list_tiktok_follower_extractions";
pub const QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME: &str = "extract_tiktok_followers";
pub const LIST_OUTLOOK_MESSAGES_TOOL_NAME: &str = "list_outlook_messages";
pub const LIST_CALENDAR_EVENTS_TOOL_NAME: &str = "list_calendar_events";
pub const SEND_OUTLOOK_EMAIL_TOOL_NAME: &str = "send_outlook_email";
pub const CREATE_CALENDAR_EVENT_TOOL_NAME: &str = "create_calendar_event";
pub const UPDATE_CALENDAR_EVENT_TOOL_NAME: &str = "update_calendar_event";
pub const MCP_SERVER_NAME: &str = "cst_chat";
pub const MCP_BEARER_ENV: &str = "CST_CHAT_AUTONOMOUS_TOOL_TOKEN";
const CAPABILITY_TTL_SECONDS: i64 = 2 * 60 * 60;
const MAX_TOOL_CALLS_PER_TURN: u16 = 8;
const MAX_GOAL_TOOL_CALLS_PER_SESSION: u16 = 512;
const MAX_CHAT_CREATIONS_PER_TURN: u8 = 1;
/// Sous-quota des actions qui sortent de l'application (e-mail, agenda, TikTok). Le
/// budget global de 8 appels est partage avec les outils d'agents : sans ce
/// plafond, une boucle de redaction epuiserait le tour a elle seule.
const MAX_EXTERNAL_ACTIONS_PER_TURN: u8 = 3;
const CHAT_OPEN_REQUEST_TTL_SECONDS: i64 = 2 * 60 * 60;
const MAX_PENDING_CHAT_OPEN_REQUESTS: usize = 32;
const MAX_CHAT_PROMPT_LENGTH: usize = 32_768;

#[derive(Debug, Clone)]
pub(crate) struct ChatModelToolServerConfig {
    pub url: String,
    pub bearer_token: String,
}

/// Etendue des outils accordee a un porteur de capacite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatToolScope {
    /// Chat interactif : un humain lit la reponse et peut reagir.
    Full,
    /// Cycle d'agent autonome : uniquement les outils qui touchent aux donnees
    /// personnelles de son proprietaire. Un agent qui tourne seul, sans personne
    /// devant l'ecran, n'a rien a faire avec la creation d'autres agents ou
    /// l'ouverture de chats — il se dupliquerait sans controle.
    PersonalDataOnly,
    /// Terminal Freebuff interactif : uniquement l'etat Goal lie a ce
    /// proprietaire, ce compte et ce panneau/workspace.
    GoalsOnly,
}

/// Outils qui travaillent sur le compte Microsoft de l'utilisateur.
pub(crate) const MICROSOFT_TOOL_NAMES: [&str; 5] = [
    LIST_OUTLOOK_MESSAGES_TOOL_NAME,
    LIST_CALENDAR_EVENTS_TOOL_NAME,
    SEND_OUTLOOK_EMAIL_TOOL_NAME,
    CREATE_CALENDAR_EVENT_TOOL_NAME,
    UPDATE_CALENDAR_EVENT_TOOL_NAME,
];
pub(crate) const PRIVATE_MESSAGE_TOOL_NAMES: [&str; 4] = [
    LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME,
    LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME,
    CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME,
    CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME,
];
pub(crate) const TIKTOK_DM_TOOL_NAMES: [&str; 8] = [
    LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME,
    LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME,
    MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME,
    SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME,
    PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
    SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
    LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME,
    QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME,
];
pub(crate) const TERMINAL_GOAL_TOOL_NAMES: [&str; 3] = [
    CREATE_GOAL_TOOL_NAME,
    GET_GOAL_TOOL_NAME,
    UPDATE_GOAL_TOOL_NAME,
];

impl ChatToolScope {
    pub fn allows(&self, tool_name: &str) -> bool {
        match self {
            // Un chat normal recoit le Goal autonome de Switch. get_goal et
            // update_goal sont reserves au TUI Freebuff, dont le processus
            // interactif reprend lui-meme le travail entre deux appels.
            Self::Full => ![GET_GOAL_TOOL_NAME, UPDATE_GOAL_TOOL_NAME].contains(&tool_name),
            Self::PersonalDataOnly => {
                MICROSOFT_TOOL_NAMES.contains(&tool_name)
                    || PRIVATE_MESSAGE_TOOL_NAMES.contains(&tool_name)
                    || TIKTOK_DM_TOOL_NAMES.contains(&tool_name)
            }
            Self::GoalsOnly => TERMINAL_GOAL_TOOL_NAMES.contains(&tool_name),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AutonomousAgentToolContext {
    pub account_id: String,
    pub scope: ChatToolScope,
    /// Compte utilisateur nominatif (`AuthIdentity.id`) qui a demarre le tour.
    /// `None` pour un tour lance avec le jeton administrateur. C'est la seule
    /// cle acceptable pour retrouver une boite Microsoft : `account_id` designe
    /// un profil CLI, partageable entre plusieurs personnes.
    pub user_id: Option<String>,
    pub source_chat_key: Option<String>,
    pub project_dir: Option<String>,
    pub mode: ChatTurnMode,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    /// Cle opaque de persistance des goals. Le modele ne peut jamais la
    /// fournir ni la remplacer.
    pub goal_key: Option<String>,
}

#[derive(Debug, Clone)]
struct ChatToolCapability {
    context: AutonomousAgentToolContext,
    expires_at: i64,
    calls: u16,
    chat_creations: u8,
    external_actions: u8,
}

impl ChatToolCapability {
    fn max_calls(&self) -> u16 {
        match self.context.scope {
            ChatToolScope::GoalsOnly => MAX_GOAL_TOOL_CALLS_PER_SESSION,
            ChatToolScope::Full | ChatToolScope::PersonalDataOnly => MAX_TOOL_CALLS_PER_TURN,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct ChatToolCapabilityRegistry {
    inner: Arc<Mutex<HashMap<String, ChatToolCapability>>>,
}

impl ChatToolCapabilityRegistry {
    pub fn issue(&self, context: AutonomousAgentToolContext) -> Result<String, String> {
        let now = metrics::now_ts();
        let expires_at = if context.scope == ChatToolScope::GoalsOnly {
            i64::MAX
        } else {
            now.saturating_add(CAPABILITY_TTL_SECONDS)
        };
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let mut entries = self
            .inner
            .lock()
            .map_err(|_| "Registre des outils du chat verrouille".to_string())?;
        entries.retain(|_, entry| entry.expires_at > now);
        entries.insert(
            token.clone(),
            ChatToolCapability {
                context,
                expires_at,
                calls: 0,
                chat_creations: 0,
                external_actions: 0,
            },
        );
        Ok(token)
    }

    pub fn authorize(&self, token: &str) -> Result<AutonomousAgentToolContext, String> {
        let now = metrics::now_ts();
        let mut entries = self
            .inner
            .lock()
            .map_err(|_| "Registre des outils du chat verrouille".to_string())?;
        entries.retain(|_, entry| entry.expires_at > now);
        entries
            .get(token)
            .map(|entry| entry.context.clone())
            .ok_or_else(|| "Capacite MCP absente ou expiree".to_string())
    }

    pub fn claim_call(&self, token: &str) -> Result<AutonomousAgentToolContext, String> {
        let now = metrics::now_ts();
        let mut entries = self
            .inner
            .lock()
            .map_err(|_| "Registre des outils du chat verrouille".to_string())?;
        entries.retain(|_, entry| entry.expires_at > now);
        let entry = entries
            .get_mut(token)
            .ok_or_else(|| "Capacite MCP absente ou expiree".to_string())?;
        if entry.calls >= entry.max_calls() {
            return Err("Limite d'actions autonomes atteinte pour ce tour".to_string());
        }
        entry.calls = entry.calls.saturating_add(1);
        Ok(entry.context.clone())
    }

    pub fn claim_chat_creation(&self, token: &str) -> Result<AutonomousAgentToolContext, String> {
        let now = metrics::now_ts();
        let mut entries = self
            .inner
            .lock()
            .map_err(|_| "Registre des outils du chat verrouille".to_string())?;
        entries.retain(|_, entry| entry.expires_at > now);
        let entry = entries
            .get_mut(token)
            .ok_or_else(|| "Capacite MCP absente ou expiree".to_string())?;
        if entry.calls >= MAX_TOOL_CALLS_PER_TURN {
            return Err("Limite d'actions autonomes atteinte pour ce tour".to_string());
        }
        if entry.chat_creations >= MAX_CHAT_CREATIONS_PER_TURN {
            return Err("Un seul nouveau chat peut etre cree par tour".to_string());
        }
        entry.calls = entry.calls.saturating_add(1);
        entry.chat_creations = entry.chat_creations.saturating_add(1);
        Ok(entry.context.clone())
    }

    /// Consomme un appel ET une action externe. Utilise par les outils qui
    /// preparent un e-mail, une ecriture d'agenda ou une campagne TikTok. Ces
    /// actions suivent leur propre confirmation, mais chaque brouillon coute a
    /// l'utilisateur une carte a relire.
    pub fn claim_external_action(&self, token: &str) -> Result<AutonomousAgentToolContext, String> {
        let now = metrics::now_ts();
        let mut entries = self
            .inner
            .lock()
            .map_err(|_| "Registre des outils du chat verrouille".to_string())?;
        entries.retain(|_, entry| entry.expires_at > now);
        let entry = entries
            .get_mut(token)
            .ok_or_else(|| "Capacite MCP absente ou expiree".to_string())?;
        if entry.calls >= MAX_TOOL_CALLS_PER_TURN {
            return Err("Limite d'actions autonomes atteinte pour ce tour".to_string());
        }
        if entry.external_actions >= MAX_EXTERNAL_ACTIONS_PER_TURN {
            return Err(
                "Limite d'actions externes atteinte pour ce tour ; traite les brouillons en attente avant d'en preparer d'autres"
                    .to_string(),
            );
        }
        entry.calls = entry.calls.saturating_add(1);
        entry.external_actions = entry.external_actions.saturating_add(1);
        Ok(entry.context.clone())
    }

    pub fn revoke(&self, token: &str) {
        if let Ok(mut entries) = self.inner.lock() {
            entries.remove(token);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateChatToolArguments {
    pub prompt: String,
    #[serde(default)]
    pub mode: Option<ChatTurnMode>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatOpenRequest {
    pub id: String,
    pub account_id: String,
    pub source_chat_key: Option<String>,
    pub project_dir: Option<String>,
    pub mode: ChatTurnMode,
    pub prompt: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub created_at: i64,
}

impl CreateChatToolArguments {
    pub fn into_request(
        self,
        context: &AutonomousAgentToolContext,
    ) -> Result<ChatOpenRequest, String> {
        let prompt = self.prompt.trim().to_string();
        if prompt.is_empty() {
            return Err("Le message initial du nouveau chat est requis".to_string());
        }
        if prompt.chars().count() > MAX_CHAT_PROMPT_LENGTH {
            return Err(format!(
                "Le message initial depasse {MAX_CHAT_PROMPT_LENGTH} caracteres"
            ));
        }
        Ok(ChatOpenRequest {
            id: Uuid::new_v4().to_string(),
            account_id: context.account_id.clone(),
            source_chat_key: context.source_chat_key.clone(),
            project_dir: context.project_dir.clone(),
            mode: self.mode.unwrap_or(context.mode),
            prompt,
            model: context.model.clone(),
            reasoning_effort: context.reasoning_effort.clone(),
            created_at: metrics::now_ts(),
        })
    }
}

#[derive(Clone, Default)]
pub(crate) struct ChatOpenRequestRegistry {
    inner: Arc<Mutex<VecDeque<ChatOpenRequest>>>,
}

impl ChatOpenRequestRegistry {
    pub fn enqueue(&self, request: ChatOpenRequest) -> Result<ChatOpenRequest, String> {
        let now = metrics::now_ts();
        let mut requests = self
            .inner
            .lock()
            .map_err(|_| "File d'ouverture des chats indisponible".to_string())?;
        requests.retain(|item| item.created_at + CHAT_OPEN_REQUEST_TTL_SECONDS >= now);
        if requests.len() >= MAX_PENDING_CHAT_OPEN_REQUESTS {
            return Err("Trop de demandes d'ouverture de chat sont en attente".to_string());
        }
        requests.push_back(request.clone());
        Ok(request)
    }

    pub fn claim(&self) -> Result<Vec<ChatOpenRequest>, String> {
        let now = metrics::now_ts();
        let mut requests = self
            .inner
            .lock()
            .map_err(|_| "File d'ouverture des chats indisponible".to_string())?;
        requests.retain(|item| item.created_at + CHAT_OPEN_REQUEST_TTL_SECONDS >= now);
        Ok(requests.drain(..).collect())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateAutonomousGoalToolArguments {
    pub objective: String,
}

impl CreateAutonomousGoalToolArguments {
    pub fn into_request(
        self,
        context: AutonomousAgentToolContext,
    ) -> Result<CreateAutonomousAgentRequest, String> {
        CreateAutonomousAgentToolArguments {
            objective: self.objective,
            name: None,
            role: None,
            mode: None,
            interval_minutes: None,
            require_user_review: None,
            mobile_notifications_enabled: None,
            initial_memory: None,
            test_command: None,
            test_timeout_seconds: None,
        }
        .into_request(context)
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CreateTerminalGoalToolArguments {
    pub objective: String,
    #[serde(default)]
    pub token_budget: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct UpdateGoalToolArguments {
    pub status: TerminalGoalStatus,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateAutonomousAgentToolArguments {
    pub objective: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub mode: Option<ChatTurnMode>,
    #[serde(default)]
    pub interval_minutes: Option<u64>,
    #[serde(default)]
    pub require_user_review: Option<bool>,
    #[serde(default)]
    pub mobile_notifications_enabled: Option<bool>,
    #[serde(default)]
    pub initial_memory: Option<String>,
    #[serde(default)]
    pub test_command: Option<String>,
    #[serde(default)]
    pub test_timeout_seconds: Option<u64>,
}

impl CreateAutonomousAgentToolArguments {
    pub fn into_request(
        self,
        context: AutonomousAgentToolContext,
    ) -> Result<CreateAutonomousAgentRequest, String> {
        let interval_seconds = self
            .interval_minutes
            .map(|minutes| {
                minutes
                    .checked_mul(60)
                    .ok_or_else(|| "Frequence autonome invalide".to_string())
            })
            .transpose()?;
        Ok(CreateAutonomousAgentRequest {
            name: self.name,
            objective: self.objective,
            role: self.role,
            source_chat_key: context.source_chat_key,
            source_proposal_id: None,
            source_report_id: None,
            source_report_idea_index: None,
            account_id: context.account_id,
            // L'agent cree depuis un chat herite du proprietaire de ce chat :
            // il travaillera plus tard sur la boite de cette personne, pas sur
            // une autre, et un tour administrateur n'en designe aucune.
            owner_id: context.user_id,
            project_dir: context.project_dir,
            mode: self.mode.unwrap_or(context.mode),
            require_user_review: self.require_user_review.unwrap_or(true),
            model: context.model,
            reasoning_effort: context.reasoning_effort,
            connectors: Vec::<ChatAppConnector>::new(),
            whatsapp_notification_channel_id: None,
            telegram_notification_channel_id: None,
            mobile_notifications_enabled: self.mobile_notifications_enabled.unwrap_or(false),
            interval_seconds,
            trigger_kind: AutonomousTriggerKind::Schedule,
            watch_paths: Vec::new(),
            debounce_seconds: None,
            allow_git_publish: false,
            initial_memory: self.initial_memory,
            test_command: self.test_command,
            test_timeout_seconds: self.test_timeout_seconds,
            defer_first_run: false,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateAutonomousAgentToolArguments {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub objective: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub clear_role: bool,
    #[serde(default)]
    pub mode: Option<ChatTurnMode>,
    #[serde(default)]
    pub interval_minutes: Option<u64>,
    #[serde(default)]
    pub require_user_review: Option<bool>,
    #[serde(default)]
    pub mobile_notifications_enabled: Option<bool>,
    #[serde(default)]
    pub test_command: Option<String>,
    #[serde(default)]
    pub clear_test_command: bool,
    #[serde(default)]
    pub test_timeout_seconds: Option<u64>,
}

impl UpdateAutonomousAgentToolArguments {
    pub fn into_request(
        self,
        agent: &AutonomousAgentSnapshot,
        activate: bool,
    ) -> Result<UpdateAutonomousAgentRequest, String> {
        let has_patch = self.name.is_some()
            || self.objective.is_some()
            || self.role.is_some()
            || self.clear_role
            || self.mode.is_some()
            || self.interval_minutes.is_some()
            || self.require_user_review.is_some()
            || self.mobile_notifications_enabled.is_some()
            || self.test_command.is_some()
            || self.clear_test_command
            || self.test_timeout_seconds.is_some();
        if !has_patch {
            return Err("Indique au moins une modification a appliquer".to_string());
        }
        if self.role.is_some() && self.clear_role {
            return Err("role et clearRole ne peuvent pas etre utilises ensemble".to_string());
        }
        if self.test_command.is_some() && self.clear_test_command {
            return Err(
                "testCommand et clearTestCommand ne peuvent pas etre utilises ensemble".to_string(),
            );
        }

        let interval_seconds = self
            .interval_minutes
            .map(|minutes| {
                minutes
                    .checked_mul(60)
                    .ok_or_else(|| "Frequence autonome invalide".to_string())
            })
            .transpose()?
            .unwrap_or(agent.interval_seconds);
        let role = if self.clear_role {
            None
        } else {
            self.role.or_else(|| agent.role.clone())
        };
        let test_command = if self.clear_test_command {
            None
        } else {
            self.test_command.or_else(|| agent.test_command.clone())
        };

        Ok(UpdateAutonomousAgentRequest {
            name: Some(self.name.unwrap_or_else(|| agent.name.clone())),
            objective: self.objective.unwrap_or_else(|| agent.objective.clone()),
            role,
            account_id: Some(agent.account_id.clone()),
            project_dir: agent.project_dir.clone(),
            mode: self.mode.unwrap_or(agent.mode),
            require_user_review: self
                .require_user_review
                .unwrap_or(agent.require_user_review),
            model: agent.model.clone(),
            reasoning_effort: agent.reasoning_effort.clone(),
            connectors: agent.connectors.clone(),
            whatsapp_notification_channel_id: None,
            telegram_notification_channel_id: None,
            mobile_notifications_enabled: self.mobile_notifications_enabled,
            interval_seconds: Some(interval_seconds),
            trigger_kind: Some(agent.trigger_kind),
            watch_paths: Some(agent.watch_paths.clone()),
            debounce_seconds: Some(agent.debounce_seconds),
            allow_git_publish: Some(agent.allow_git_publish),
            test_command,
            test_timeout_seconds: Some(
                self.test_timeout_seconds
                    .unwrap_or(agent.test_timeout_seconds),
            ),
            activate,
        })
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutonomousAgentPolicyScope {
    CurrentProject,
    Account,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ApplyAutonomousAgentPolicyToolArguments {
    pub instruction: String,
    #[serde(default)]
    pub require_visual_evidence: bool,
    #[serde(default)]
    pub scope: Option<AutonomousAgentPolicyScope>,
}

impl ApplyAutonomousAgentPolicyToolArguments {
    pub fn resolved_scope(
        &self,
        _context: &AutonomousAgentToolContext,
    ) -> AutonomousAgentPolicyScope {
        self.scope
            .unwrap_or(AutonomousAgentPolicyScope::CurrentProject)
    }
}

fn windows_style_project_path(value: &str) -> bool {
    value.contains('\\')
        || value
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':')
}

fn same_project(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .trim()
            .replace('\\', "/")
            .trim_end_matches('/')
            .to_string()
    };
    let left_normalized = normalize(left);
    let right_normalized = normalize(right);
    if windows_style_project_path(left) || windows_style_project_path(right) {
        left_normalized.eq_ignore_ascii_case(&right_normalized)
    } else {
        left_normalized == right_normalized
    }
}

pub(crate) fn agents_for_policy_context(
    agents: &[AutonomousAgentSnapshot],
    context: &AutonomousAgentToolContext,
    scope: AutonomousAgentPolicyScope,
) -> Result<Vec<AutonomousAgentSnapshot>, String> {
    let project_dir = match scope {
        AutonomousAgentPolicyScope::CurrentProject => Some(
            context
                .project_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    "Ce chat n'a pas de projet courant pour limiter la politique. Utilise la portee account uniquement si l'utilisateur vise explicitement tous ses projets."
                        .to_string()
                })?,
        ),
        AutonomousAgentPolicyScope::Account => None,
    };
    let mut selected = agents
        .iter()
        .filter(|agent| {
            !agent.system_managed
                && agent.account_id == context.account_id
                && matches!(
                    agent.status,
                    AutonomousAgentStatus::Active | AutonomousAgentStatus::NeedsAttention
                )
                && (agent.require_user_review
                    || agent.pending_review.is_some()
                    || agent.approved_review.is_some())
                && project_dir.is_none_or(|project| {
                    agent
                        .project_dir
                        .as_deref()
                        .is_some_and(|agent_project| same_project(agent_project, project))
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    selected.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| right.created_at.cmp(&left.created_at))
    });
    if selected.is_empty() {
        return Err(match scope {
            AutonomousAgentPolicyScope::CurrentProject => {
                "Aucun agent autonome actif avec review humaine n'est rattache au compte et au projet courants"
                    .to_string()
            }
            AutonomousAgentPolicyScope::Account => {
                "Aucun agent autonome actif avec review humaine n'est disponible pour ce compte"
                    .to_string()
            }
        });
    }
    Ok(selected)
}

pub(crate) fn linked_agent_for_context(
    agents: &[AutonomousAgentSnapshot],
    context: &AutonomousAgentToolContext,
) -> Result<AutonomousAgentSnapshot, String> {
    let source_chat_key = context.source_chat_key.as_deref().ok_or_else(|| {
        "Ce chat n'a pas de cle persistante permettant de retrouver son agent autonome".to_string()
    })?;
    agents
        .iter()
        .filter(|agent| {
            !agent.system_managed
                && agent.account_id == context.account_id
                && agent.owner_id == context.user_id
                && agent.source_chat_key.as_deref() == Some(source_chat_key)
        })
        .max_by_key(|agent| (agent.created_at, agent.updated_at))
        .cloned()
        .ok_or_else(|| {
            "Ce chat n'a pas d'agent autonome lie. Cree-le d'abord depuis ce chat.".to_string()
        })
}

pub(crate) fn initialize_response(
    id: Value,
    requested_version: Option<&str>,
    scope: ChatToolScope,
) -> Value {
    let protocol_version = requested_version
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("2025-06-18");
    if scope == ChatToolScope::GoalsOnly {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": protocol_version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "codex-switch-terminal-freebuff-goals",
                    "title": "Goals persistants du terminal Freebuff",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Ces outils appartiennent uniquement au terminal Freebuff courant. Cree un goal seulement pour un objectif long explicitement demande, poursuis-le jusqu'a son resultat reel, consulte get_goal lors d'une reprise, et n'appelle update_goal avec complete que lorsque plus aucun travail requis ne reste. blocked exige le meme blocage pendant au moins trois tentatives consecutives sans autre progres possible."
            }
        });
    }
    if scope == ChatToolScope::PersonalDataOnly {
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": protocol_version,
                "capabilities": { "tools": { "listChanged": false } },
                "serverInfo": {
                    "name": "codex-switch-terminal-chat-tools",
                    "title": "Outils du chat Codex Switch Terminal",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "instructions": "Ces outils donnent acces a la messagerie interne, au connecteur TikMatrix local et au compte Microsoft 365 du proprietaire. L'authentification TikTok reste exclusivement dans l'application TikTok de l'appareil Android : ne demande et ne transmets jamais de mot de passe, cookie, code 2FA ou captcha au VPS. Quand l'utilisateur veut connecter un emetteur, appelle manage_tiktok_sender_login avec open_login : la fenetre TikTok locale s'ouvre et l'utilisateur y saisit lui-meme ses secrets et validations. Dis-lui ensuite de revenir confirmer que la connexion est terminee ; seulement alors appelle le meme outil avec match_accounts, puis list_tiktok_sender_accounts et select_tiktok_sender_account. La session persistante est celle de l'application TikTok/TikMatrix locale, aucun token TikTok n'est extrait par le chat. Utilise list_tiktok_sender_accounts pour verifier les comptes reconnus. Utilise select_tiktok_sender_account pour choisir l'emetteur ; un seul compte actif par appareil est exige pour garantir son identite. Quand l'utilisateur donne dans une meme demande un @username destinataire, le message exact et un ordre explicite d'envoi, appelle directement send_tiktok_dm_campaign avec recipient et message : cet outil utilise l'emetteur selectionne, prepare et met en file l'envoi en une seule operation, sans demander une seconde confirmation. Utilise prepare_tiktok_dm_campaign seulement si l'utilisateur demande un brouillon ou si le destinataire, le texte exact ou l'intention d'envoyer manque. L'envoi direct est limite a un destinataire et ne doit jamais servir a de la prospection non sollicitee. extract_tiktok_followers accepte une demande unique allant jusqu'a 1000 resultats pour un compte appartenant a l'utilisateur ou explicitement autorise. TikTok peut toutefois ne rendre qu'environ 50 profils visibles : presente toujours le nombre effectivement retourne et ne promets jamais une liste exhaustive. Son option dmPipeline ne conserve que l'intersection avec une liste explicite d'au plus cinq comptes secondaires confirmes comme controles et cree uniquement un brouillon. Apres la collecte, affiche les destinataires et le message exact, puis attends une nouvelle confirmation humaine avant send_tiktok_dm_campaign. N'envoie jamais aux autres noms extraits. Pour une diffusion interne, commence par list_private_message_users et cree normalement un brouillon. Les outils d'ecriture Microsoft ne font que preparer des cartes a confirmer. Indique clairement le statut reel de chaque action et distingue l'acceptation de la tache par TikMatrix d'une livraison confirmee par TikTok."
            }
        });
    }
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "protocolVersion": protocol_version,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": "codex-switch-terminal-chat-tools",
                "title": "Outils du chat Codex Switch Terminal",
                "version": env!("CARGO_PKG_VERSION")
            },
            "instructions": "Utilise les outils d'agents et de chats uniquement sur demande explicite. N'affirme jamais qu'une creation, une modification ou une mise en pause a reussi avant le succes de l'outil. Utilise list_control_devices avant de viser un appareil Android ou iOS. N'appelle control_device pour une action qui modifie l'appareil que sur demande explicite de l'utilisateur et ne mets confirmed a true qu'apres sa confirmation explicite. Pour l'action shell, montre la commande exacte et attends sa confirmation explicite avant l'appel confirme. Si control_device retourne queued ou claimed, suis uniquement cet identifiant avec get_control_device_action et ne duplique jamais l'action. L'authentification TikTok reste sur Windows et l'appareil Android ; ne demande jamais de secret TikTok dans le chat. Pour connecter un emetteur, utilise manage_tiktok_sender_login avec open_login, demande a l'utilisateur d'effectuer localement le login/captcha/2FA et d'indiquer quand il a termine, puis utilise match_accounts. Verifie ensuite les emetteurs avec list_tiktok_sender_accounts et choisis-en un avec select_tiktok_sender_account. Aucun token TikTok n'est extrait : la session est conservee par l'application locale. Quand une demande explicite contient un @username destinataire et le message exact, send_tiktok_dm_campaign peut utiliser l'emetteur selectionne, preparer et mettre en file cet envoi unique directement avec recipient et message, sans seconde confirmation. Utilise prepare_tiktok_dm_campaign pour un brouillon ou une demande incomplete. Refuse toute prospection non sollicitee et indique si le connecteur Windows est hors ligne. La collecte de followers accepte une demande unique jusqu'a 1000 noms pour un compte appartenant a l'utilisateur ou explicitement autorise. TikTok peut cependant limiter la liste visible a environ 50 profils : annonce le nombre reellement retourne et ne qualifie jamais le resultat d'exhaustif sans preuve. L'option dmPipeline de la collecte peut preparer un brouillon uniquement pour l'intersection avec une liste explicite d'au plus cinq comptes secondaires confirmes comme controles. N'utilise jamais les autres noms extraits. Affiche le brouillon termine puis attends une nouvelle confirmation humaine avant send_tiktok_dm_campaign. La messagerie interne conserve son propre flux de brouillon et de consentement. Les outils Microsoft lisent la boite et l'agenda lies au compte ; leurs outils d'ecriture ne font que preparer une carte que l'utilisateur doit confirmer. Distingue toujours l'acceptation par TikMatrix d'une livraison confirmee par TikTok."
        }
    })
}

pub(crate) fn tools_list_response(id: Value, scope: ChatToolScope) -> Value {
    if scope == ChatToolScope::GoalsOnly {
        return goal_tools_response(id);
    }
    let mut response = all_tools_response(id);
    if scope != ChatToolScope::Full {
        if let Some(tools) = response
            .pointer_mut("/result/tools")
            .and_then(Value::as_array_mut)
        {
            tools.retain(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| scope.allows(name))
            });
        }
    }
    response
}

fn goal_tools_response(id: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": CREATE_GOAL_TOOL_NAME,
                    "title": "Creer un goal persistant",
                    "description": "Cree le goal actif du terminal Freebuff pour un objectif long. Echoue si un goal inacheve existe deja. token_budget ne doit etre fourni que si l'utilisateur a explicitement fixe ce budget.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "objective": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 32768,
                                "description": "Objectif concret a poursuivre jusqu'a sa completion reelle."
                            },
                            "token_budget": {
                                "type": "integer",
                                "minimum": 1,
                                "description": "Budget facultatif, uniquement lorsque l'utilisateur l'a explicitement demande."
                            }
                        },
                        "required": ["objective"]
                    },
                    "annotations": {
                        "title": "Creer un goal",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": GET_GOAL_TOOL_NAME,
                    "title": "Lire le goal courant",
                    "description": "Retourne le goal persistant du terminal Freebuff courant et son statut, ou indique qu'aucun goal n'existe.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Lire le goal",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": UPDATE_GOAL_TOOL_NAME,
                    "title": "Terminer ou bloquer le goal",
                    "description": "Met le goal actif a complete uniquement quand l'objectif est reellement atteint, ou a blocked seulement apres trois tentatives consecutives confrontees au meme blocage sans progres possible.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "status": {
                                "type": "string",
                                "enum": ["complete", "blocked"]
                            }
                        },
                        "required": ["status"]
                    },
                    "annotations": {
                        "title": "Mettre a jour le goal",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                }
            ]
        }
    })
}

fn all_tools_response(id: Value) -> Value {
    let output_schema = json!({
        "type": "object",
        "properties": {
            "agentId": { "type": "string" },
            "name": { "type": "string" },
            "status": { "type": "string" },
            "sourceChatKey": { "type": ["string", "null"] }
        },
        "required": ["agentId", "name", "status", "sourceChatKey"]
    });
    let policy_output_schema = json!({
        "type": "object",
        "properties": {
            "updatedCount": { "type": "integer" },
            "failedCount": { "type": "integer" },
            "agents": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agentId": { "type": "string" },
                        "name": { "type": "string" },
                        "status": { "type": "string" }
                    },
                    "required": ["agentId", "name", "status"]
                }
            },
            "failures": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "agentId": { "type": "string" },
                        "name": { "type": "string" },
                        "error": { "type": "string" }
                    },
                    "required": ["agentId", "name", "error"]
                }
            }
        },
        "required": ["updatedCount", "failedCount", "agents", "failures"]
    });
    let general_report_output_schema = json!({
        "type": "object",
        "properties": {
            "enabled": { "type": "boolean" },
            "pendingUnreadReports": { "type": "integer" },
            "scheduled": { "type": "boolean" },
            "status": { "type": "string" }
        },
        "required": ["enabled", "pendingUnreadReports", "scheduled", "status"]
    });
    let chat_output_schema = json!({
        "type": "object",
        "properties": {
            "requestId": { "type": "string" },
            "status": { "type": "string" },
            "mode": { "type": "string" },
            "sourceChatKey": { "type": ["string", "null"] }
        },
        "required": ["requestId", "status", "mode", "sourceChatKey"]
    });
    let messages_output_schema = json!({
        "type": "object",
        "properties": {
            "count": { "type": "integer" },
            "messages": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "subject": { "type": "string" },
                        "from": { "type": "string" },
                        "fromName": { "type": "string" },
                        "receivedAt": { "type": "string" },
                        "preview": { "type": "string" },
                        "isRead": { "type": "boolean" },
                        "hasAttachments": { "type": "boolean" }
                    },
                    "required": ["id", "subject", "from", "receivedAt", "preview"]
                }
            }
        },
        "required": ["count", "messages"]
    });
    let events_output_schema = json!({
        "type": "object",
        "properties": {
            "count": { "type": "integer" },
            "window": {
                "type": "object",
                "properties": {
                    "start": { "type": "string" },
                    "end": { "type": "string" },
                    "timeZone": { "type": "string" }
                },
                "required": ["start", "end", "timeZone"]
            },
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": { "type": "string" },
                        "subject": { "type": "string" },
                        "start": { "type": "string" },
                        "end": { "type": "string" },
                        "timeZone": { "type": "string" },
                        "location": { "type": "string" },
                        "organizer": { "type": "string" },
                        "attendees": { "type": "array", "items": { "type": "string" } },
                        "isAllDay": { "type": "boolean" },
                        "isCancelled": { "type": "boolean" }
                    },
                    "required": ["id", "subject", "start", "end", "timeZone"]
                }
            }
        },
        "required": ["count", "events", "window"]
    });
    let pending_action_output_schema = json!({
        "type": "object",
        "properties": {
            "actionId": { "type": "string" },
            "status": { "type": "string" },
            "kind": { "type": "string" },
            "summary": { "type": "string" }
        },
        "required": ["actionId", "status", "kind", "summary"]
    });
    let instant_description =
        "Date et heure ISO 8601 avec decalage horaire explicite, par exemple 2026-07-25T14:00:00+02:00. Un instant sans decalage est refuse.";
    // Selecteur de boite : l'utilisateur peut lier plusieurs comptes Microsoft.
    // Le champ ne designe QUE l'une de SES boites deja liees ; une adresse
    // inconnue est refusee, jamais utilisee comme un compte tiers.
    let account_property = json!({
        "type": "string",
        "description": "Adresse de l'une de vos boites Microsoft liees, si vous en avez plusieurs. Par defaut : votre boite principale. Ne mets ici qu'une de tes propres adresses liees, jamais celle d'un destinataire."
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "tools": [
                {
                    "name": CREATE_GOAL_TOOL_NAME,
                    "title": "Creer un goal durable",
                    "description": "Cree et demarre un goal persistant lie au chat courant. Le goal poursuit l'objectif dans le temps avec un agent autonome Switch qui herite du compte, du modele et de l'environnement du chat. Utilise cet outil uniquement lorsque l'utilisateur demande explicitement un goal. Ne simule jamais sa creation et n'annonce sa reussite qu'apres le succes de l'outil.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "objective": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 32768,
                                "description": "Objectif durable, precis, verifiable et directement actionnable."
                            }
                        },
                        "required": ["objective"]
                    },
                    "outputSchema": output_schema.clone(),
                    "annotations": {
                        "title": "Creer un goal durable",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": AUTONOMOUS_AGENT_TOOL_NAME,
                    "title": "Creer un agent autonome",
                    "description": "Cree et demarre un nouvel agent autonome persistant, lie au chat courant. Utilise cet outil lorsque l'utilisateur demande explicitement de creer, lancer, demarrer ou rendre autonome un agent pour poursuivre une mission dans le temps. Le compte, le modele et l'environnement sont ceux du chat et ne doivent pas etre demandes a nouveau. Ne l'utilise pas pour modifier un agent existant, une simple explication ou une simulation.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "objective": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 32768,
                                "description": "Mission durable, precise et directement actionnable par l'agent."
                            },
                            "name": {
                                "type": "string",
                                "maxLength": 120,
                                "description": "Nom court facultatif."
                            },
                            "role": {
                                "type": "string",
                                "maxLength": 4000,
                                "description": "Specialite, responsabilites et limites facultatives."
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["build", "plan", "ask"],
                                "description": "build pour agir, plan pour analyser sans modifier, ask pour conseiller."
                            },
                            "intervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10080,
                                "description": "Delai entre deux cycles. Par defaut : 15 minutes."
                            },
                            "requireUserReview": {
                                "type": "boolean",
                                "description": "Par defaut true : demander une validation avant d'appliquer les changements."
                            },
                            "mobileNotificationsEnabled": {
                                "type": "boolean",
                                "description": "Envoie les nouveaux comptes rendus et les alertes dans l'app mobile configuree."
                            },
                            "initialMemory": {
                                "type": "string",
                                "maxLength": 2000,
                                "description": "Contraintes et decisions durables utiles, sans recopier toute la conversation."
                            },
                            "testCommand": {
                                "type": "string",
                                "maxLength": 8000,
                                "description": "Commande de validation facultative dans l'environnement courant."
                            },
                            "testTimeoutSeconds": {
                                "type": "integer",
                                "minimum": 5,
                                "maximum": 1800
                            }
                        },
                        "required": ["objective"]
                    },
                    "outputSchema": output_schema.clone(),
                    "annotations": {
                        "title": "Creer un agent autonome",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": UPDATE_AUTONOMOUS_AGENT_TOOL_NAME,
                    "title": "Modifier l'agent autonome de ce chat",
                    "description": "Modifie uniquement l'agent autonome le plus recent lie au chat courant. Utilise cet outil lorsque l'utilisateur demande explicitement de changer son nom, son objectif, son role, son mode, sa frequence, ses notifications mobiles, sa validation humaine ou sa commande de test. Ne demande et n'accepte aucun identifiant d'agent. Les champs absents conservent leur valeur actuelle. Ne l'utilise pas pour creer un nouvel agent ni pour cibler l'agent d'un autre chat.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "minProperties": 1,
                        "properties": {
                            "name": {
                                "type": "string",
                                "maxLength": 120,
                                "description": "Nouveau nom court."
                            },
                            "objective": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 32768,
                                "description": "Nouvelle mission durable et directement actionnable."
                            },
                            "role": {
                                "type": "string",
                                "maxLength": 4000,
                                "description": "Nouveau role. Ne pas combiner avec clearRole."
                            },
                            "clearRole": {
                                "type": "boolean",
                                "description": "true pour supprimer le role existant."
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["build", "plan", "ask"]
                            },
                            "intervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10080,
                                "description": "Nouveau delai entre deux cycles."
                            },
                            "requireUserReview": {
                                "type": "boolean",
                                "description": "Active ou desactive la validation humaine."
                            },
                            "mobileNotificationsEnabled": {
                                "type": "boolean",
                                "description": "Active ou desactive les rapports et alertes dans l'app mobile."
                            },
                            "testCommand": {
                                "type": "string",
                                "maxLength": 8000,
                                "description": "Nouvelle commande de validation. Ne pas combiner avec clearTestCommand."
                            },
                            "clearTestCommand": {
                                "type": "boolean",
                                "description": "true pour supprimer la commande de validation."
                            },
                            "testTimeoutSeconds": {
                                "type": "integer",
                                "minimum": 5,
                                "maximum": 1800
                            }
                        }
                    },
                    "outputSchema": output_schema.clone(),
                    "annotations": {
                        "title": "Modifier l'agent autonome de ce chat",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": PAUSE_AUTONOMOUS_AGENT_TOOL_NAME,
                    "title": "Mettre en pause l'agent autonome de ce chat",
                    "description": "Met en pause uniquement l'agent autonome le plus recent lie au chat courant. La pause arrete son cycle ou sa validation en cours et supprime sa prochaine planification jusqu'a une reprise explicite depuis l'interface. Utilise cet outil seulement lorsque l'utilisateur demande explicitement de mettre cet agent en pause. Ne demande et n'accepte aucun identifiant d'agent. Un agent deja en pause reste en pause sans erreur.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "outputSchema": output_schema.clone(),
                    "annotations": {
                        "title": "Mettre en pause l'agent autonome de ce chat",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME,
                    "title": "Activer le compte rendu general du superviseur",
                    "description": "Active et, s'il existe des comptes rendus non lus, planifie immediatement le compte rendu general du superviseur. La synthese compile toutes les sources sans doublon et les classe par priorite critique, haute, moyenne puis basse. Utilise cet outil lorsque l'utilisateur demande explicitement cette capacite ou une synthese generale ; il fonctionne depuis n'importe quel chat et n'accepte aucun identifiant d'agent.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "outputSchema": general_report_output_schema,
                    "annotations": {
                        "title": "Activer le compte rendu general",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME,
                    "title": "Appliquer une regle aux agents actifs",
                    "description": "Ajoute sans l'ecraser une instruction durable aux agents autonomes actifs qui utilisent deja la review humaine, invalide leurs anciennes autorisations et relance proprement les cycles concernes. Utilise cet outil seulement si l'utilisateur demande explicitement d'appliquer une meme politique a plusieurs agents deja actifs, notamment une preuve visuelle avant/apres. La portee par defaut est le compte et le projet courants ; utilise account uniquement si l'utilisateur vise explicitement tous ses projets de ce compte. L'outil n'accepte aucun identifiant d'agent et ne modifie ni objectif, ni role, ni frequence.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "instruction": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 2000,
                                "description": "Regle durable, precise et verifiable a ajouter sans remplacer les missions existantes."
                            },
                            "scope": {
                                "type": "string",
                                "enum": ["current_project", "account"],
                                "description": "current_project par defaut. account seulement sur demande explicite visant tous les projets du compte."
                            },
                            "requireVisualEvidence": {
                                "type": "boolean",
                                "description": "true pour une politique visuelle : une image reelle sous .codex-proof devient obligatoire avant autorisation. false pour une regle non visuelle."
                            }
                        },
                        "required": ["instruction", "requireVisualEvidence"]
                    },
                    "outputSchema": policy_output_schema,
                    "annotations": {
                        "title": "Appliquer une regle aux agents actifs",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": CREATE_CHAT_TOOL_NAME,
                    "title": "Ouvrir un nouveau chat",
                    "description": "Ouvre dans l'interface web un chat normal separe et lui envoie le message initial. Utilise cet outil uniquement quand l'utilisateur demande explicitement d'ouvrir, creer ou lancer un autre chat. Le compte, le modele, l'effort de raisonnement et l'environnement sont herites du chat courant et ne doivent pas etre redemandes. Un seul nouveau chat peut etre cree par tour. Ne l'utilise pas pour une question theorique sur les chats ni pour creer un agent autonome.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "prompt": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 32768,
                                "description": "Message initial complet a envoyer dans le nouveau chat."
                            },
                            "mode": {
                                "type": "string",
                                "enum": ["build", "plan", "ask"],
                                "description": "Mode facultatif. Par defaut, reprend le mode du chat courant."
                            }
                        },
                        "required": ["prompt"]
                    },
                    "outputSchema": chat_output_schema,
                    "annotations": {
                        "title": "Ouvrir un nouveau chat",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": LIST_PRIVATE_MESSAGE_USERS_TOOL_NAME,
                    "title": "Lister les destinataires internes",
                    "description": "Liste les utilisateurs de Codex Switch Terminal auxquels le proprietaire peut ecrire. Reutilise les identifiants retournes tels quels ; n'invente jamais un destinataire.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Lister les destinataires internes",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": LIST_PRIVATE_MESSAGE_CAMPAIGNS_TOOL_NAME,
                    "title": "Suivre les campagnes internes",
                    "description": "Liste les campagnes de messages du proprietaire avec leur statut, progression, cadence et erreurs. Utilise cet outil avant de piloter une campagne existante.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Suivre les campagnes internes",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME,
                    "title": "Creer une campagne de messages internes",
                    "description": "Cree une campagne persistante. Commence par list_private_message_users. Les variantes tournent entre les destinataires et acceptent {{username}}, {{index}} et {{total}}. Cree un brouillon par defaut. startImmediately et consentConfirmed ne peuvent etre true que si le consentement de toute cette audience est explicitement etabli dans la demande ou la memoire.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "name": { "type": "string", "maxLength": 120 },
                            "recipientIds": {
                                "type": "array",
                                "minItems": 1,
                                "maxItems": 100,
                                "uniqueItems": true,
                                "items": { "type": "string" }
                            },
                            "messageVariants": {
                                "type": "array",
                                "minItems": 1,
                                "maxItems": 20,
                                "items": { "type": "string", "minLength": 1, "maxLength": 4000 }
                            },
                            "intervalSeconds": {
                                "type": "integer",
                                "minimum": 5,
                                "maximum": 3600
                            },
                            "startImmediately": { "type": "boolean" },
                            "consentConfirmed": { "type": "boolean" },
                            "idempotencyKey": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 160,
                                "pattern": "^[A-Za-z0-9._:-]+$",
                                "description": "Cle stable propre a cette campagne pour qu'une relance de l'outil ne cree jamais de doublon."
                            }
                        },
                        "required": ["recipientIds", "messageVariants", "idempotencyKey"]
                    },
                    "annotations": {
                        "title": "Creer une campagne interne",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": CONTROL_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME,
                    "title": "Piloter une campagne interne",
                    "description": "Demarre, met en pause, reprend ou annule une campagne existante du proprietaire. Recupere d'abord son identifiant et son statut avec list_private_message_campaigns. start exige un consentement confirme ; cancel arrete seulement les livraisons encore en attente.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "campaignId": { "type": "string", "minLength": 1 },
                            "action": {
                                "type": "string",
                                "enum": ["start", "pause", "resume", "cancel"]
                            }
                        },
                        "required": ["campaignId", "action"]
                    },
                    "annotations": {
                        "title": "Piloter une campagne interne",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": LIST_CONTROL_DEVICES_TOOL_NAME,
                    "title": "Lister les appareils controles",
                    "description": "Liste les appareils Android et iOS actuellement visibles par le connecteur de controle, avec leur identifiant, leur etat et leurs capacites. Utilise cet outil avant control_device afin de selectionner un appareil reel et disponible.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Lister les appareils controles",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": CONTROL_DEVICE_TOOL_NAME,
                    "title": "Controler un appareil",
                    "description": "Execute une action sur un appareil Android ou iOS explicitement selectionne. Les actions qui modifient l'appareil (open_screen, tap, swipe, type_text, key_event, open_app et shell) exigent une demande explicite de l'utilisateur et confirmed=true seulement apres sa confirmation explicite. Pour shell, presente toujours la commande exacte, attends sa confirmation explicite, puis transmets exactement cette commande sans l'etendre ni la reformuler.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "deviceId": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 256,
                                "description": "Identifiant exact retourne par list_control_devices."
                            },
                            "action": {
                                "type": "string",
                                "enum": ["info", "screenshot", "open_screen", "tap", "swipe", "type_text", "key_event", "open_app", "shell"]
                            },
                            "args": {
                                "type": "object",
                                "description": "Arguments propres a l'action. N'ajoute aucun argument qui n'a pas ete demande ou necessaire a l'action selectionnee."
                            },
                            "confirmed": {
                                "type": "boolean",
                                "description": "Vrai uniquement apres confirmation explicite par l'utilisateur de l'action mutante ; pour shell, cette confirmation doit porter sur la commande exacte."
                            }
                        },
                        "required": ["deviceId", "action"]
                    },
                    "annotations": {
                        "title": "Controler un appareil",
                        "readOnlyHint": false,
                        "destructiveHint": true,
                        "idempotentHint": false,
                        "openWorldHint": false
                    }
                },
                {
                    "name": GET_CONTROL_DEVICE_ACTION_TOOL_NAME,
                    "title": "Suivre une action appareil",
                    "description": "Lit l'etat et le resultat d'une action deja retournee par control_device. Utilise cet outil lorsqu'une action est queued ou claimed ; ne soumets jamais une seconde action identique pour contourner un delai d'attente.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "actionId": {
                                "type": "string",
                                "minLength": 36,
                                "maxLength": 36,
                                "pattern": "^[0-9a-fA-F-]{36}$"
                            }
                        },
                        "required": ["actionId"]
                    },
                    "annotations": {
                        "title": "Suivre une action appareil",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME,
                    "title": "Suivre les campagnes TikTok de test",
                    "description": "Liste les brouillons et soumissions TikTok du proprietaire, ainsi que l'etat recent du connecteur Windows TikMatrix. Un statut submitted signifie que TikMatrix a accepte la tache, pas que TikTok a garanti chaque livraison.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Suivre les campagnes TikTok",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME,
                    "title": "Verifier les comptes emetteurs TikTok",
                    "description": "Liste les comptes TikTok reconnus par le TikMatrix Windows, leur appareil, leur etat de connexion et le compte emetteur selectionne. Aucun mot de passe, cookie ou code 2FA n'est lu ni stocke sur le VPS. Si la liste est vide, demande a l'utilisateur de se connecter dans l'application TikTok de l'appareil Android puis de lancer Match Accounts dans TikMatrix.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Verifier les emetteurs TikTok",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME,
                    "title": "Connecter un compte emetteur TikTok",
                    "description": "Pilote l'appareil Android local sans transmettre de secret au VPS. Utilise open_scrcpy pour afficher l'appareil USB, l'emulateur ou l'appareil ADB reseau choisi dans une fenetre scrcpy locale. Utilise open_login pour ouvrir TikTok sur cet appareil. L'utilisateur saisit ensuite ses informations et resout les verifications uniquement dans cette fenetre locale. Quand il confirme avoir termine, utilise match_accounts pour demander a TikMatrix de detecter la session persistante.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "action": {
                                "type": "string",
                                "enum": ["open_scrcpy", "open_login", "match_accounts"]
                            },
                            "deviceSerial": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 96,
                                "description": "Facultatif lorsqu'un seul appareil Android autorise est connecte par ADB."
                            }
                        },
                        "required": ["action"]
                    },
                    "annotations": {
                        "title": "Gerer la connexion TikTok locale",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME,
                    "title": "Selectionner le compte emetteur TikTok",
                    "description": "Associe au proprietaire un compte TikTok deja connecte et reconnu par TikMatrix. Le compte doit etre actif sur un appareil connecte. Un seul compte actif est accepte par appareil afin de garantir l'identite de l'emetteur.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "username": {
                                "type": "string",
                                "minLength": 2,
                                "maxLength": 25,
                                "pattern": "^@?[A-Za-z0-9._]{2,24}$"
                            }
                        },
                        "required": ["username"]
                    },
                    "annotations": {
                        "title": "Selectionner l'emetteur TikTok",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
                    "title": "Preparer des messages TikTok de test",
                    "description": "Cree un brouillon persistant, sans envoyer. Accepte au maximum cinq @username appartenant explicitement a l'utilisateur et un message unique sur une ligne. Montre toujours l'apercu exact avant de demander ou d'utiliser la confirmation d'envoi. N'utilise jamais cet outil pour de la prospection non sollicitee.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "recipients": {
                                "type": "array",
                                "minItems": 1,
                                "maxItems": 5,
                                "uniqueItems": true,
                                "items": {
                                    "type": "string",
                                    "minLength": 2,
                                    "maxLength": 25,
                                    "pattern": "^@?[A-Za-z0-9._]{2,24}$"
                                }
                            },
                            "message": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 500,
                                "description": "Message exact, sur une seule ligne."
                            },
                            "minIntervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10,
                                "default": 1
                            },
                            "maxIntervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10,
                                "default": 2
                            },
                            "deviceSerial": {
                                "type": "string",
                                "maxLength": 96,
                                "description": "Facultatif lorsqu'un seul appareil TikMatrix est connecte."
                            },
                            "senderAccount": {
                                "type": "string",
                                "minLength": 2,
                                "maxLength": 25,
                                "pattern": "^@?[A-Za-z0-9._]{2,24}$",
                                "description": "Compte emetteur TikTok reconnu par TikMatrix. Facultatif si un emetteur par defaut est selectionne."
                            },
                            "idempotencyKey": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 160,
                                "pattern": "^[A-Za-z0-9._:-]+$",
                                "description": "Cle stable propre a cet apercu pour eviter tout doublon."
                            }
                        },
                        "required": ["recipients", "message", "idempotencyKey"]
                    },
                    "annotations": {
                        "title": "Preparer une campagne TikTok",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": false
                    }
                },
                {
                    "name": SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
                    "title": "Envoyer un message TikTok",
                    "description": "Pour un ordre explicite contenant un destinataire et le message exact, utilise directement recipient et message : le VPS prepare puis met l'envoi dans la file TikMatrix en une operation. L'ancien format campaignId reste accepte pour envoyer un brouillon deja prepare. N'utilise jamais cet outil pour une demande ambigue ou de la prospection non sollicitee.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "recipient": {
                                "type": "string",
                                "minLength": 2,
                                "maxLength": 25,
                                "pattern": "^@?[A-Za-z0-9._]{2,24}$",
                                "description": "Compte TikTok explicitement nomme par l'utilisateur."
                            },
                            "message": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 500,
                                "description": "Message exact demande par l'utilisateur, sur une seule ligne."
                            },
                            "minIntervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10,
                                "default": 1
                            },
                            "maxIntervalMinutes": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 10,
                                "default": 2
                            },
                            "deviceSerial": {
                                "type": "string",
                                "maxLength": 96,
                                "description": "Facultatif lorsqu'un seul appareil TikMatrix est connecte."
                            },
                            "senderAccount": {
                                "type": "string",
                                "minLength": 2,
                                "maxLength": 25,
                                "pattern": "^@?[A-Za-z0-9._]{2,24}$",
                                "description": "Compte emetteur TikTok reconnu par TikMatrix. Facultatif si un emetteur par defaut est selectionne."
                            },
                            "idempotencyKey": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 160,
                                "pattern": "^[A-Za-z0-9._:-]+$",
                                "description": "Facultatif ; le VPS en genere une pour ce tour si elle manque."
                            },
                            "campaignId": {
                                "type": "string",
                                "minLength": 36,
                                "maxLength": 36
                            },
                            "ownedAccountsConfirmed": { "type": "boolean" },
                            "sendConfirmed": { "type": "boolean" }
                        },
                        "oneOf": [
                            {
                                "required": ["recipient", "message"]
                            },
                            {
                                "required": [
                                    "campaignId",
                                    "ownedAccountsConfirmed",
                                    "sendConfirmed"
                                ]
                            }
                        ]
                    },
                    "annotations": {
                        "title": "Envoyer un message TikTok",
                        "readOnlyHint": false,
                        "destructiveHint": true,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME,
                    "title": "Lire les collectes de followers TikTok",
                    "description": "Liste les collectes de followers du proprietaire. Quand le statut est completed, le champ usernames contient les @username effectivement lus dans le fichier produit par TikMatrix.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {}
                    },
                    "annotations": {
                        "title": "Lire les collectes TikTok",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME,
                    "title": "Extraire des followers TikTok",
                    "description": "Demarre en une seule operation TikMatrix la collecte de jusqu'a 1000 @username disponibles dans les followers d'un compte, puis dedoublonne le resultat. Utilise cet outil seulement si l'utilisateur affirme que le compte source lui appartient ou qu'il est autorise a l'utiliser. TikTok peut ne rendre qu'environ 50 profils visibles : rapporte toujours le nombre reellement obtenu et ne promets pas une liste exhaustive. L'option dmPipeline relie la collecte a la messagerie sans prospection : elle intersecte le resultat avec au plus cinq comptes secondaires explicitement fournis et confirmes comme controles, puis cree seulement un brouillon. Affiche ensuite son apercu et attends une nouvelle confirmation humaine avant tout envoi.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "targetUsername": {
                                "type": "string",
                                "minLength": 2,
                                "maxLength": 25,
                                "pattern": "^@?[A-Za-z0-9._]{2,24}$"
                            },
                            "maxCount": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 1000,
                                "default": 1000,
                                "description": "Maximum demande a TikMatrix en un passage. Utilise 1000 lorsque l'utilisateur demande tous les followers disponibles."
                            },
                            "deviceSerial": {
                                "type": "string",
                                "maxLength": 96,
                                "description": "Facultatif lorsqu'un seul appareil TikMatrix est connecte."
                            },
                            "authorizedAccountConfirmed": {
                                "type": "boolean",
                                "description": "Vrai uniquement si l'utilisateur a explicitement confirme etre proprietaire du compte source ou autorise a l'utiliser."
                            },
                            "dmPipeline": {
                                "type": "object",
                                "additionalProperties": false,
                                "description": "Facultatif. Prepare apres la collecte un brouillon de DM uniquement pour les comptes controles de cette liste qui apparaissent dans le resultat. Ne jamais remplir cette liste avec les autres followers extraits.",
                                "properties": {
                                    "ownedRecipientAllowlist": {
                                        "type": "array",
                                        "minItems": 1,
                                        "maxItems": 5,
                                        "uniqueItems": true,
                                        "items": {
                                            "type": "string",
                                            "minLength": 2,
                                            "maxLength": 25,
                                            "pattern": "^@?[A-Za-z0-9._]{2,24}$"
                                        },
                                        "description": "Liste explicite de comptes secondaires controles par l'utilisateur, jamais la liste scrapee complete."
                                    },
                                    "message": {
                                        "type": "string",
                                        "minLength": 1,
                                        "maxLength": 500,
                                        "pattern": "^[^\\r\\n]+$",
                                        "description": "Message exact du futur brouillon."
                                    },
                                    "ownedAccountsConfirmed": {
                                        "type": "boolean",
                                        "description": "Vrai uniquement si l'utilisateur confirme explicitement controler tous les comptes de ownedRecipientAllowlist."
                                    }
                                },
                                "required": [
                                    "ownedRecipientAllowlist",
                                    "message",
                                    "ownedAccountsConfirmed"
                                ]
                            },
                            "idempotencyKey": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 160,
                                "pattern": "^[A-Za-z0-9._:-]+$",
                                "description": "Cle stable pour eviter de lancer deux fois la meme collecte."
                            }
                        },
                        "required": [
                            "targetUsername",
                            "maxCount",
                            "authorizedAccountConfirmed",
                            "idempotencyKey"
                        ]
                    },
                    "annotations": {
                        "title": "Extraire des followers TikTok",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": LIST_OUTLOOK_MESSAGES_TOOL_NAME,
                    "title": "Lire la boite Outlook de l'utilisateur",
                    "description": "Liste les e-mails de la boite Microsoft 365 liee au compte de l'utilisateur connecte. Utilise cet outil quand l'utilisateur demande de consulter, chercher, resumer ou verifier ses e-mails. La boite est deduite du compte connecte : ne demande jamais d'adresse, d'identifiant ni de mot de passe. Les corps complets ne sont pas retournes : appuie-toi sur l'objet, l'expediteur et l'apercu.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "query": {
                                "type": "string",
                                "maxLength": 200,
                                "description": "Recherche plein texte facultative, par exemple « facture » ou « from:jean@example.fr ». Sans guillemet."
                            },
                            "folder": {
                                "type": "string",
                                "enum": ["inbox", "sentitems", "drafts", "archive"],
                                "description": "Dossier a lire. Par defaut : inbox."
                            },
                            "limit": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 25,
                                "description": "Nombre de messages a retourner. Par defaut : 10."
                            },
                            "account": account_property.clone()
                        }
                    },
                    "outputSchema": messages_output_schema,
                    "annotations": {
                        "title": "Lire la boite Outlook",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": LIST_CALENDAR_EVENTS_TOOL_NAME,
                    "title": "Consulter l'agenda Microsoft de l'utilisateur",
                    "description": "Liste les evenements de l'agenda Microsoft 365 lie au compte de l'utilisateur connecte, y compris les occurrences des series recurrentes. Utilise cet outil quand l'utilisateur demande son planning, ses rendez-vous, ses disponibilites ou un creneau libre. Les horaires renvoyes sont en UTC : convertis-les avant de les presenter. L'agenda est deduit du compte connecte et ne doit jamais etre demande.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "start": {
                                "type": "string",
                                "description": instant_description
                            },
                            "end": {
                                "type": "string",
                                "description": instant_description
                            },
                            "limit": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 50,
                                "description": "Nombre d'evenements a retourner. Par defaut : 20."
                            },
                            "account": account_property.clone()
                        }
                    },
                    "outputSchema": events_output_schema,
                    "annotations": {
                        "title": "Consulter l'agenda Microsoft",
                        "readOnlyHint": true,
                        "destructiveHint": false,
                        "idempotentHint": true,
                        "openWorldHint": true
                    }
                },
                {
                    "name": SEND_OUTLOOK_EMAIL_TOOL_NAME,
                    "title": "Preparer un e-mail Outlook a confirmer",
                    "description": "Prepare un e-mail depuis la boite Microsoft 365 de l'utilisateur connecte et l'affiche dans la conversation pour validation. CET OUTIL N'ENVOIE RIEN : l'e-mail ne part que si l'utilisateur clique sur Envoyer dans la carte de confirmation. Ne dis donc jamais que le message est parti ; annonce qu'il attend sa validation. Utilise cet outil quand l'utilisateur demande d'ecrire ou d'envoyer un e-mail. L'expediteur est le compte lie et ne doit jamais etre demande.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "to": {
                                "type": "array",
                                "minItems": 1,
                                "maxItems": 25,
                                "items": { "type": "string" },
                                "description": "Adresses des destinataires. Utilise uniquement des adresses fournies par l'utilisateur ou lues dans ses e-mails : n'en invente aucune."
                            },
                            "cc": {
                                "type": "array",
                                "maxItems": 25,
                                "items": { "type": "string" },
                                "description": "Adresses en copie."
                            },
                            "subject": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 255,
                                "description": "Objet du message."
                            },
                            "body": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 20000,
                                "description": "Corps du message en texte brut, redige et pret a partir, dans la langue de l'utilisateur."
                            },
                            "account": account_property.clone()
                        },
                        "required": ["to", "subject", "body"]
                    },
                    "outputSchema": pending_action_output_schema.clone(),
                    "annotations": {
                        "title": "Preparer un e-mail Outlook",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": true
                    }
                },
                {
                    "name": CREATE_CALENDAR_EVENT_TOOL_NAME,
                    "title": "Preparer un evenement d'agenda a confirmer",
                    "description": "Prepare un evenement dans l'agenda Microsoft 365 de l'utilisateur connecte et l'affiche dans la conversation pour validation. CET OUTIL NE CREE RIEN : l'evenement n'est ajoute et les invitations ne partent que si l'utilisateur confirme. Annonce donc une proposition en attente, jamais un rendez-vous cree. Verifie les disponibilites avec list_calendar_events avant de proposer un creneau.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "properties": {
                            "subject": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 255,
                                "description": "Titre de l'evenement."
                            },
                            "start": {
                                "type": "string",
                                "description": instant_description
                            },
                            "end": {
                                "type": "string",
                                "description": instant_description
                            },
                            "attendees": {
                                "type": "array",
                                "maxItems": 25,
                                "items": { "type": "string" },
                                "description": "Adresses des participants a inviter. N'en invente aucune."
                            },
                            "location": {
                                "type": "string",
                                "maxLength": 255,
                                "description": "Lieu affiche dans l'invitation."
                            },
                            "body": {
                                "type": "string",
                                "maxLength": 20000,
                                "description": "Description en texte brut."
                            },
                            "onlineMeeting": {
                                "type": "boolean",
                                "description": "true pour joindre un lien Teams a l'invitation."
                            },
                            "account": account_property.clone()
                        },
                        "required": ["subject", "start", "end"]
                    },
                    "outputSchema": pending_action_output_schema.clone(),
                    "annotations": {
                        "title": "Preparer un evenement d'agenda",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": true
                    }
                },
                {
                    "name": UPDATE_CALENDAR_EVENT_TOOL_NAME,
                    "title": "Preparer la modification d'un evenement",
                    "description": "Prepare la modification d'un evenement existant de l'agenda Microsoft 365 et l'affiche dans la conversation pour validation. CET OUTIL NE MODIFIE RIEN sans confirmation de l'utilisateur. L'identifiant provient obligatoirement d'un appel prealable a list_calendar_events : ne l'invente jamais et ne le demande pas a l'utilisateur. Un deplacement d'horaire exige start et end ensemble. Cet outil ne supprime ni n'annule un evenement.",
                    "inputSchema": {
                        "type": "object",
                        "additionalProperties": false,
                        "minProperties": 2,
                        "properties": {
                            "eventId": {
                                "type": "string",
                                "minLength": 1,
                                "maxLength": 512,
                                "description": "Identifiant retourne par list_calendar_events."
                            },
                            "subject": {
                                "type": "string",
                                "maxLength": 255,
                                "description": "Nouveau titre."
                            },
                            "start": {
                                "type": "string",
                                "description": instant_description
                            },
                            "end": {
                                "type": "string",
                                "description": instant_description
                            },
                            "location": {
                                "type": "string",
                                "maxLength": 255,
                                "description": "Nouveau lieu."
                            },
                            "body": {
                                "type": "string",
                                "maxLength": 20000,
                                "description": "Nouvelle description en texte brut."
                            },
                            "account": account_property
                        },
                        "required": ["eventId"]
                    },
                    "outputSchema": pending_action_output_schema,
                    "annotations": {
                        "title": "Preparer la modification d'un evenement",
                        "readOnlyHint": false,
                        "destructiveHint": false,
                        "idempotentHint": false,
                        "openWorldHint": true
                    }
                }
            ]
        }
    })
}

/// Reponse des outils Microsoft en lecture seule. Le texte est ce que le modele
/// lit ; `structuredContent` porte les donnees exploitables.
pub(crate) fn tool_microsoft_data_response(id: Value, text: &str, structured: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

/// Reponse des outils Microsoft qui sortent de l'application. Le texte insiste
/// sur le fait que rien n'est parti : c'est la seule protection contre un
/// modele qui annoncerait un envoi imaginaire a l'utilisateur.
pub(crate) fn tool_pending_action_response(
    id: Value,
    action: &crate::microsoft::PendingMicrosoftAction,
) -> Value {
    if action.requires_link {
        // Le brouillon est garde : dire au modele d'en rediger un autre ferait
        // perdre le travail et agacerait l'utilisateur pour rien. Deux cas
        // distincts, a ne pas confondre : soit aucune boite utilisable n'existe
        // (il faut en connecter/relier une), soit la boite VISEE est a relier
        // alors qu'une autre boite saine existe (relier celle-ci, ou changer
        // d'expediteur sur la carte).
        let text = if action.has_other_usable_account {
            let mailbox = action
                .account_email
                .as_deref()
                .unwrap_or("la boite choisie");
            format!(
                "Brouillon conserve : {}. La boite {} doit etre reliee avant de pouvoir l'utiliser, mais l'utilisateur a d'autres boites Microsoft actives. Sur la carte affichee, il peut soit relier {}, soit choisir une autre de ses boites comme expediteur, puis confirmer. Ne prepare pas de nouveau brouillon et n'affirme rien tant qu'il n'a pas confirme.",
                action.summary, mailbox, mailbox
            )
        } else {
            format!(
                "Brouillon conserve : {}. Aucune boite Microsoft utilisable n'est liee : l'interface propose a l'utilisateur de connecter ou relier son compte dans la conversation, et le brouillon partira apres cela et sa confirmation. Invite-le a utiliser la carte affichee, et ne prepare pas de nouveau brouillon.",
                action.summary
            )
        };
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": text }],
                "structuredContent": {
                    "actionId": action.id,
                    "status": "awaiting_account_link",
                    "kind": action.kind,
                    "summary": action.summary,
                },
                "isError": false
            }
        });
    }
    let text = match action.kind.as_str() {
        "sendEmail" => format!(
            "Brouillon prepare : {}. L'e-mail N'EST PAS envoye : une carte de confirmation attend la validation de l'utilisateur dans la conversation.",
            action.summary
        ),
        "createEvent" => format!(
            "Proposition preparee : {}. L'evenement N'EST PAS cree tant que l'utilisateur n'a pas confirme dans la conversation.",
            action.summary
        ),
        _ => format!(
            "Modification preparee : {}. Elle N'EST PAS appliquee tant que l'utilisateur n'a pas confirme dans la conversation.",
            action.summary
        ),
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "structuredContent": {
                "actionId": action.id,
                "status": "awaiting_confirmation",
                "kind": action.kind,
                "summary": action.summary,
            },
            "isError": false
        }
    })
}

pub(crate) fn tool_chat_open_response(id: Value, request: &ChatOpenRequest) -> Value {
    let structured = json!({
        "requestId": request.id,
        "status": "queued",
        "mode": request.mode,
        "sourceChatKey": request.source_chat_key,
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": "Le nouveau chat a ete demande. L'interface web va l'ouvrir et envoyer le message initial."
            }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_success_response(id: Value, agent: &AutonomousAgentSnapshot) -> Value {
    let structured = json!({
        "agentId": agent.id,
        "name": agent.name,
        "status": agent.status,
        "sourceChatKey": agent.source_chat_key,
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": format!(
                    "Agent autonome « {} » cree et demarre avec succes (id: {}). Il est lie au chat courant.",
                    agent.name, agent.id
                )
            }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_autonomous_goal_success_response(
    id: Value,
    agent: &AutonomousAgentSnapshot,
) -> Value {
    let structured = json!({
        "agentId": agent.id,
        "name": agent.name,
        "status": agent.status,
        "sourceChatKey": agent.source_chat_key,
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": format!(
                    "Goal « {} » cree et demarre avec succes (id: {}). Il est persistant et lie au chat courant.",
                    agent.name, agent.id
                )
            }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_terminal_goal_success_response(
    id: Value,
    action: &str,
    goal: &TerminalGoalSnapshot,
) -> Value {
    let status = match goal.status {
        TerminalGoalStatus::Active => "active",
        TerminalGoalStatus::Blocked => "blocked",
        TerminalGoalStatus::Complete => "complete",
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": format!("Goal Switch {action} avec succes (statut: {status}).")
            }],
            "structuredContent": goal,
            "isError": false
        }
    })
}

pub(crate) fn tool_terminal_goal_get_response(
    id: Value,
    goal: Option<&TerminalGoalSnapshot>,
) -> Value {
    let text = match goal {
        Some(goal) => format!("Goal Switch courant : {}", goal.objective),
        None => "Aucun goal Switch n'existe pour ce terminal.".to_string(),
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": text }],
            "structuredContent": { "goal": goal },
            "isError": false
        }
    })
}

pub(crate) fn tool_update_success_response(id: Value, agent: &AutonomousAgentSnapshot) -> Value {
    let structured = json!({
        "agentId": agent.id,
        "name": agent.name,
        "status": agent.status,
        "sourceChatKey": agent.source_chat_key,
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{
                "type": "text",
                "text": format!(
                    "Agent autonome « {} » modifie avec succes (id: {}). Son etat est {}.",
                    agent.name,
                    agent.id,
                    match agent.status {
                        crate::autonomous::AutonomousAgentStatus::Active => "actif",
                        crate::autonomous::AutonomousAgentStatus::Paused => "en pause",
                        crate::autonomous::AutonomousAgentStatus::Completed => "termine",
                        crate::autonomous::AutonomousAgentStatus::NeedsAttention => {
                            "en attente d'attention"
                        }
                    }
                )
            }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_pause_success_response(
    id: Value,
    agent: &AutonomousAgentSnapshot,
    already_paused: bool,
) -> Value {
    let structured = json!({
        "agentId": agent.id,
        "name": agent.name,
        "status": agent.status,
        "sourceChatKey": agent.source_chat_key,
    });
    let message = if already_paused {
        format!("L'agent autonome « {} » etait deja en pause.", agent.name)
    } else {
        format!(
            "L'agent autonome « {} » a ete mis en pause. Son cycle courant a ete arrete et aucune nouvelle execution ne sera planifiee avant sa reprise.",
            agent.name
        )
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_general_report_response(
    id: Value,
    supervisor: Option<&AutonomousAgentSnapshot>,
    pending_unread_reports: usize,
    scheduled: bool,
) -> Value {
    let status = supervisor
        .map(|agent| format!("{:?}", agent.status).to_ascii_lowercase())
        .unwrap_or_else(|| "standby".to_string());
    let structured = json!({
        "enabled": true,
        "pendingUnreadReports": pending_unread_reports,
        "scheduled": scheduled,
        "status": status,
    });
    let message = if pending_unread_reports == 0 {
        "Le compte rendu general du superviseur est actif. Aucun compte rendu non lu n'est actuellement a compiler.".to_string()
    } else if scheduled {
        format!(
            "Le compte rendu general du superviseur est actif et planifie immediatement pour compiler {pending_unread_reports} compte(s) rendu(s) non lu(s) par ordre de priorite."
        )
    } else {
        format!(
            "Le compte rendu general du superviseur est actif. {pending_unread_reports} compte(s) rendu(s) non lu(s) seront compiles au prochain passage."
        )
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "structuredContent": structured,
            "isError": false
        }
    })
}

pub(crate) fn tool_policy_response(
    id: Value,
    updated: &[AutonomousAgentSnapshot],
    failures: &[(String, String, String)],
) -> Value {
    let agents = updated
        .iter()
        .map(|agent| {
            json!({
                "agentId": agent.id,
                "name": agent.name,
                "status": agent.status,
            })
        })
        .collect::<Vec<_>>();
    let failure_values = failures
        .iter()
        .map(|(agent_id, name, error)| {
            json!({
                "agentId": agent_id,
                "name": name,
                "error": error,
            })
        })
        .collect::<Vec<_>>();
    let structured = json!({
        "updatedCount": agents.len(),
        "failedCount": failure_values.len(),
        "agents": agents,
        "failures": failure_values,
    });
    let message = if failures.is_empty() {
        format!(
            "Politique durable appliquee avec succes a {} agent(s) autonome(s). La review humaine est active et les cycles concernes repartent avec la nouvelle regle.",
            updated.len()
        )
    } else {
        format!(
            "Politique appliquee a {} agent(s), mais {} mise(s) a jour ont echoue. N'affirme pas que tous les agents ont ete modifies et rapporte les echecs.",
            updated.len(),
            failures.len()
        )
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "structuredContent": structured,
            "isError": !failures.is_empty()
        }
    })
}

pub(crate) fn tool_error_response(id: Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": message }],
            "isError": true
        }
    })
}

pub(crate) fn protocol_error(id: Value, code: i64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> AutonomousAgentToolContext {
        AutonomousAgentToolContext {
            account_id: "account-1".to_string(),
            scope: ChatToolScope::Full,
            user_id: Some("user-1".to_string()),
            source_chat_key: Some("chat-1".to_string()),
            project_dir: Some("C:/project".to_string()),
            mode: ChatTurnMode::Build,
            model: Some("gpt-test".to_string()),
            reasoning_effort: Some("high".to_string()),
            goal_key: None,
        }
    }

    fn snapshot(id: &str, created_at: i64) -> AutonomousAgentSnapshot {
        serde_json::from_value(json!({
            "id": id,
            "name": format!("Agent {id}"),
            "objective": "Objectif initial",
            "role": "Role initial",
            "sourceChatKey": "chat-1",
            "accountId": "account-1",
            "ownerId": "user-1",
            "projectDir": "C:/original",
            "mode": "plan",
            "model": "gpt-original",
            "reasoningEffort": "medium",
            "connectors": [],
            "intervalSeconds": 900,
            "triggerKind": "workspace_change",
            "watchPaths": ["src"],
            "debounceSeconds": 12,
            "allowGitPublish": true,
            "status": "paused",
            "createdAt": created_at,
            "updatedAt": created_at,
            "requireUserReview": true,
            "testCommand": "npm test",
            "testTimeoutSeconds": 120
        }))
        .unwrap()
    }

    #[test]
    fn capability_binds_the_tool_to_the_source_chat() {
        let registry = ChatToolCapabilityRegistry::default();
        let token = registry.issue(context()).unwrap();
        let claimed = registry.claim_call(&token).unwrap();
        assert_eq!(claimed.account_id, "account-1");
        assert_eq!(claimed.source_chat_key.as_deref(), Some("chat-1"));
        registry.revoke(&token);
        assert!(registry.authorize(&token).is_err());
    }

    #[test]
    fn capability_carries_the_authenticated_user() {
        let registry = ChatToolCapabilityRegistry::default();
        let token = registry.issue(context()).unwrap();
        assert_eq!(
            registry.claim_call(&token).unwrap().user_id.as_deref(),
            Some("user-1")
        );
    }

    #[test]
    fn capability_bounds_external_actions_below_the_call_budget() {
        let registry = ChatToolCapabilityRegistry::default();
        let token = registry.issue(context()).unwrap();
        for _ in 0..MAX_EXTERNAL_ACTIONS_PER_TURN {
            assert!(registry.claim_external_action(&token).is_ok());
        }
        assert!(registry
            .claim_external_action(&token)
            .unwrap_err()
            .contains("Limite d'actions externes"));
        // Le budget global reste disponible pour les autres outils.
        assert!(registry.claim_call(&token).is_ok());
    }

    #[test]
    fn capability_allows_only_one_chat_creation_per_turn() {
        let registry = ChatToolCapabilityRegistry::default();
        let token = registry.issue(context()).unwrap();
        assert!(registry.claim_chat_creation(&token).is_ok());
        assert_eq!(
            registry.claim_chat_creation(&token).unwrap_err(),
            "Un seul nouveau chat peut etre cree par tour"
        );
    }

    #[test]
    fn chat_open_request_inherits_the_source_context_and_is_claimed_once() {
        let arguments: CreateChatToolArguments = serde_json::from_value(json!({
            "prompt": "  Verifie les tests dans un chat separe.  ",
            "mode": "plan"
        }))
        .unwrap();
        let request = arguments.into_request(&context()).unwrap();
        assert_eq!(request.account_id, "account-1");
        assert_eq!(request.source_chat_key.as_deref(), Some("chat-1"));
        assert_eq!(request.project_dir.as_deref(), Some("C:/project"));
        assert_eq!(request.model.as_deref(), Some("gpt-test"));
        assert_eq!(request.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(request.mode, ChatTurnMode::Plan);
        assert_eq!(request.prompt, "Verifie les tests dans un chat separe.");

        let registry = ChatOpenRequestRegistry::default();
        registry.enqueue(request).unwrap();
        assert_eq!(registry.claim().unwrap().len(), 1);
        assert!(registry.claim().unwrap().is_empty());
    }

    #[test]
    fn tool_arguments_cannot_override_account_or_environment() {
        let goal: CreateAutonomousGoalToolArguments = serde_json::from_value(json!({
            "objective": "Livrer le correctif OX Alpha"
        }))
        .unwrap();
        let goal_request = goal.into_request(context()).unwrap();
        assert_eq!(goal_request.objective, "Livrer le correctif OX Alpha");
        assert_eq!(goal_request.account_id, "account-1");
        assert_eq!(goal_request.source_chat_key.as_deref(), Some("chat-1"));
        assert_eq!(goal_request.project_dir.as_deref(), Some("C:/project"));
        assert_eq!(goal_request.model.as_deref(), Some("gpt-test"));
        assert!(goal_request.require_user_review);
        assert!(
            serde_json::from_value::<CreateAutonomousGoalToolArguments>(json!({
                "objective": "Objectif",
                "accountId": "account-2"
            }))
            .is_err()
        );

        let arguments: CreateAutonomousAgentToolArguments = serde_json::from_value(json!({
            "objective": "Surveiller les regressions",
            "intervalMinutes": 30
        }))
        .unwrap();
        let request = arguments.into_request(context()).unwrap();
        assert_eq!(request.account_id, "account-1");
        assert_eq!(request.source_chat_key.as_deref(), Some("chat-1"));
        assert_eq!(request.project_dir.as_deref(), Some("C:/project"));
        assert_eq!(request.interval_seconds, Some(1800));
        assert!(request.require_user_review);
        // L'agent herite du proprietaire du chat : c'est ce qui lui donnera plus
        // tard acces a la boite mail de cette personne, et d'aucune autre.
        assert_eq!(request.owner_id.as_deref(), Some("user-1"));

        // Un tour lance au jeton administrateur ne designe personne : l'agent
        // cree ne doit alors heriter d'aucune boite.
        let mut anonymous = context();
        anonymous.user_id = None;
        let arguments: CreateAutonomousAgentToolArguments = serde_json::from_value(json!({
            "objective": "Surveiller les regressions"
        }))
        .unwrap();
        assert!(arguments
            .into_request(anonymous)
            .unwrap()
            .owner_id
            .is_none());
    }

    #[test]
    fn update_arguments_apply_a_patch_without_moving_the_agent() {
        let agent = snapshot("agent-1", 10);
        let arguments: UpdateAutonomousAgentToolArguments = serde_json::from_value(json!({
            "objective": "Nouvel objectif",
            "intervalMinutes": 30,
            "clearRole": true
        }))
        .unwrap();
        let request = arguments.into_request(&agent, false).unwrap();
        assert_eq!(request.objective, "Nouvel objectif");
        assert_eq!(request.role, None);
        assert_eq!(request.account_id.as_deref(), Some("account-1"));
        assert_eq!(request.project_dir.as_deref(), Some("C:/original"));
        assert_eq!(request.mode, ChatTurnMode::Plan);
        assert_eq!(request.model.as_deref(), Some("gpt-original"));
        assert_eq!(request.interval_seconds, Some(1800));
        assert_eq!(
            request.trigger_kind,
            Some(AutonomousTriggerKind::WorkspaceChange)
        );
        assert_eq!(request.watch_paths, Some(vec!["src".to_string()]));
        assert_eq!(request.allow_git_publish, Some(true));
        assert_eq!(request.test_command.as_deref(), Some("npm test"));
        assert!(!request.activate);
    }

    #[test]
    fn update_tool_selects_only_the_newest_agent_linked_to_this_chat() {
        let older = snapshot("older", 10);
        let newer = snapshot("newer", 20);
        let mut other_chat = snapshot("other-chat", 30);
        other_chat.source_chat_key = Some("chat-2".to_string());
        let mut other_account = snapshot("other-account", 40);
        other_account.account_id = "account-2".to_string();
        let mut other_owner = snapshot("other-owner", 50);
        other_owner.owner_id = Some("user-2".to_string());
        let selected = linked_agent_for_context(
            &[older, newer, other_chat, other_account, other_owner],
            &context(),
        )
        .unwrap();
        assert_eq!(selected.id, "newer");
    }

    #[test]
    fn policy_tool_selects_running_agents_without_a_source_chat_key() {
        let mut context = context();
        context.source_chat_key = None;
        let arguments: ApplyAutonomousAgentPolicyToolArguments = serde_json::from_value(json!({
            "instruction": "Regle partagee",
            "requireVisualEvidence": true
        }))
        .unwrap();
        assert!(arguments.require_visual_evidence);
        assert_eq!(
            arguments.resolved_scope(&context),
            AutonomousAgentPolicyScope::CurrentProject
        );

        let mut current_project = snapshot("current-project", 10);
        current_project.project_dir = Some("c:\\PROJECT\\".to_string());
        current_project.status = AutonomousAgentStatus::Active;
        let mut other_project = snapshot("other-project", 20);
        other_project.project_dir = Some("C:/other".to_string());
        other_project.status = AutonomousAgentStatus::NeedsAttention;
        let mut other_account = snapshot("other-account", 30);
        other_account.account_id = "account-2".to_string();
        other_account.status = AutonomousAgentStatus::Active;
        let paused = snapshot("paused", 40);

        let project_agents = agents_for_policy_context(
            &[
                current_project.clone(),
                other_project.clone(),
                other_account.clone(),
                paused.clone(),
            ],
            &context,
            AutonomousAgentPolicyScope::CurrentProject,
        )
        .unwrap();
        assert_eq!(
            project_agents
                .iter()
                .map(|agent| agent.id.as_str())
                .collect::<Vec<_>>(),
            vec!["current-project"]
        );

        let account_agents = agents_for_policy_context(
            &[current_project, other_project, other_account, paused],
            &context,
            AutonomousAgentPolicyScope::Account,
        )
        .unwrap();
        assert_eq!(account_agents.len(), 2);
        assert!(account_agents
            .iter()
            .any(|agent| agent.id == "current-project"));
        assert!(account_agents
            .iter()
            .any(|agent| agent.id == "other-project"));
    }

    #[test]
    fn mcp_schema_teaches_the_model_when_to_control_an_agent() {
        let response = tools_list_response(json!(1), ChatToolScope::Full);
        let tools = response["result"]["tools"].as_array().unwrap();
        assert!(tools.len() >= 6);
        let goal_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(CREATE_GOAL_TOOL_NAME))
            .unwrap();
        assert_eq!(goal_tool["inputSchema"]["required"], json!(["objective"]));
        assert_eq!(
            goal_tool["inputSchema"]["additionalProperties"],
            json!(false)
        );
        assert!(goal_tool["description"]
            .as_str()
            .unwrap()
            .contains("modele et de l'environnement du chat"));
        let goal_response = tool_autonomous_goal_success_response(json!(2), &snapshot("goal", 10));
        assert_eq!(goal_response["result"]["isError"], false);
        assert!(goal_response["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Goal"));
        let tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(AUTONOMOUS_AGENT_TOOL_NAME))
            .unwrap();
        assert_eq!(tool["name"], AUTONOMOUS_AGENT_TOOL_NAME);
        assert_eq!(tool["inputSchema"]["required"], json!(["objective"]));
        assert!(tool["description"]
            .as_str()
            .unwrap()
            .contains("demande explicitement"));
        let update_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(UPDATE_AUTONOMOUS_AGENT_TOOL_NAME))
            .unwrap();
        assert_eq!(update_tool["name"], UPDATE_AUTONOMOUS_AGENT_TOOL_NAME);
        assert_eq!(update_tool["inputSchema"]["minProperties"], 1);
        assert!(update_tool["inputSchema"]["properties"]
            .get("agentId")
            .is_none());
        let pause_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(PAUSE_AUTONOMOUS_AGENT_TOOL_NAME))
            .unwrap();
        assert_eq!(pause_tool["name"], PAUSE_AUTONOMOUS_AGENT_TOOL_NAME);
        assert_eq!(pause_tool["inputSchema"]["properties"], json!({}));
        assert!(pause_tool["inputSchema"]["properties"]
            .get("agentId")
            .is_none());
        assert_eq!(pause_tool["annotations"]["idempotentHint"], true);
        assert!(pause_tool["description"]
            .as_str()
            .unwrap()
            .contains("cycle ou sa validation en cours"));
        let pause_response = tool_pause_success_response(json!(2), &snapshot("paused", 10), false);
        assert_eq!(
            pause_response["result"]["structuredContent"]["status"],
            "paused"
        );
        assert_eq!(pause_response["result"]["isError"], false);
        let general_report_tool = tools
            .iter()
            .find(|tool| {
                tool["name"].as_str() == Some(ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME)
            })
            .unwrap();
        assert_eq!(
            general_report_tool["name"],
            ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME
        );
        assert_eq!(general_report_tool["inputSchema"]["properties"], json!({}));
        assert!(general_report_tool["description"]
            .as_str()
            .unwrap()
            .contains("n'importe quel chat"));
        let general_response = tool_general_report_response(json!(2), None, 3, true);
        assert_eq!(
            general_response["result"]["structuredContent"]["pendingUnreadReports"],
            3
        );
        assert_eq!(
            general_response["result"]["structuredContent"]["enabled"],
            true
        );

        let policy_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME))
            .unwrap();
        assert_eq!(policy_tool["name"], APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME);
        assert_eq!(
            policy_tool["inputSchema"]["required"],
            json!(["instruction", "requireVisualEvidence"])
        );
        assert!(policy_tool["inputSchema"]["properties"]
            .get("agentId")
            .is_none());

        let create_chat_tool = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(CREATE_CHAT_TOOL_NAME))
            .unwrap();
        assert_eq!(
            create_chat_tool["inputSchema"]["required"],
            json!(["prompt"])
        );
        assert!(create_chat_tool["inputSchema"]["properties"]
            .get("accountId")
            .is_none());
        assert!(create_chat_tool["description"]
            .as_str()
            .unwrap()
            .contains("Un seul nouveau chat"));
    }

    #[test]
    fn goal_tools_are_scoped_between_chat_and_freebuff() {
        let full = tools_list_response(json!(1), ChatToolScope::Full);
        let full_names = full["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(full_names.contains(&CREATE_GOAL_TOOL_NAME));
        assert!(!full_names.contains(&GET_GOAL_TOOL_NAME));
        assert!(!full_names.contains(&UPDATE_GOAL_TOOL_NAME));

        let goals = tools_list_response(json!(2), ChatToolScope::GoalsOnly);
        let goal_names = goals["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(goal_names.as_slice(), TERMINAL_GOAL_TOOL_NAMES.as_slice());
        assert!(ChatToolScope::GoalsOnly.allows(CREATE_GOAL_TOOL_NAME));
        assert!(!ChatToolScope::GoalsOnly.allows(CREATE_CHAT_TOOL_NAME));
        assert!(ChatToolScope::Full.allows(CREATE_GOAL_TOOL_NAME));
        assert!(!ChatToolScope::Full.allows(GET_GOAL_TOOL_NAME));

        let instructions = initialize_response(json!(3), None, ChatToolScope::GoalsOnly);
        assert!(instructions["result"]["instructions"]
            .as_str()
            .unwrap()
            .contains("get_goal"));
    }

    #[test]
    fn an_autonomous_agent_only_sees_personal_data_and_messaging_tools() {
        let response = tools_list_response(json!(1), ChatToolScope::PersonalDataOnly);
        let tools = response["result"]["tools"].as_array().unwrap();
        let names = tools
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names.len(),
            MICROSOFT_TOOL_NAMES.len()
                + PRIVATE_MESSAGE_TOOL_NAMES.len()
                + TIKTOK_DM_TOOL_NAMES.len()
        );
        for name in MICROSOFT_TOOL_NAMES {
            assert!(names.contains(&name), "{name} manquant pour un agent");
        }
        for name in PRIVATE_MESSAGE_TOOL_NAMES {
            assert!(names.contains(&name), "{name} manquant pour un agent");
        }
        for name in TIKTOK_DM_TOOL_NAMES {
            assert!(names.contains(&name), "{name} manquant pour un agent");
        }
        // Un agent autonome qui pourrait se dupliquer ou ouvrir des chats
        // echapperait a tout controle : ces outils lui restent fermes.
        for forbidden in [
            CREATE_GOAL_TOOL_NAME,
            AUTONOMOUS_AGENT_TOOL_NAME,
            UPDATE_AUTONOMOUS_AGENT_TOOL_NAME,
            PAUSE_AUTONOMOUS_AGENT_TOOL_NAME,
            APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME,
            ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME,
            CREATE_CHAT_TOOL_NAME,
            LIST_CONTROL_DEVICES_TOOL_NAME,
            CONTROL_DEVICE_TOOL_NAME,
            GET_CONTROL_DEVICE_ACTION_TOOL_NAME,
        ] {
            assert!(
                !names.contains(&forbidden),
                "{forbidden} accessible a un agent"
            );
            assert!(!ChatToolScope::PersonalDataOnly.allows(forbidden));
        }
        // La liste et le dispatch doivent refuser exactement les memes outils.
        assert!(ChatToolScope::Full.allows(CREATE_CHAT_TOOL_NAME));
        assert!(ChatToolScope::PersonalDataOnly.allows(SEND_OUTLOOK_EMAIL_TOOL_NAME));
        assert!(ChatToolScope::PersonalDataOnly.allows(CREATE_PRIVATE_MESSAGE_CAMPAIGN_TOOL_NAME));
        assert!(ChatToolScope::PersonalDataOnly.allows(SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME));
    }

    #[test]
    fn device_control_tools_are_full_scope_only_and_have_strict_schemas() {
        let response = tools_list_response(json!(1), ChatToolScope::Full);
        let tools = response["result"]["tools"].as_array().unwrap();
        let list = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(LIST_CONTROL_DEVICES_TOOL_NAME))
            .expect("outil de liste des appareils absent");
        assert_eq!(list["inputSchema"]["additionalProperties"], false);
        assert_eq!(list["inputSchema"]["properties"], json!({}));
        assert_eq!(list["annotations"]["readOnlyHint"], true);

        let control = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(CONTROL_DEVICE_TOOL_NAME))
            .expect("outil de controle des appareils absent");
        assert_eq!(control["inputSchema"]["additionalProperties"], false);
        assert_eq!(
            control["inputSchema"]["required"],
            json!(["deviceId", "action"])
        );
        assert_eq!(
            control["inputSchema"]["properties"]["deviceId"]["minLength"],
            1
        );
        assert_eq!(
            control["inputSchema"]["properties"]["deviceId"]["maxLength"],
            256
        );
        assert_eq!(
            control["inputSchema"]["properties"]["action"]["enum"],
            json!([
                "info",
                "screenshot",
                "open_screen",
                "tap",
                "swipe",
                "type_text",
                "key_event",
                "open_app",
                "shell"
            ])
        );
        let description = control["description"].as_str().unwrap();
        assert!(description.contains("demande explicite"));
        assert!(description.contains("commande exacte"));
        assert!(!ChatToolScope::PersonalDataOnly.allows(LIST_CONTROL_DEVICES_TOOL_NAME));
        assert!(!ChatToolScope::PersonalDataOnly.allows(CONTROL_DEVICE_TOOL_NAME));
        let status = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(GET_CONTROL_DEVICE_ACTION_TOOL_NAME))
            .expect("outil de suivi des actions appareil absent");
        assert_eq!(status["inputSchema"]["additionalProperties"], false);
        assert_eq!(status["inputSchema"]["required"], json!(["actionId"]));
        assert_eq!(status["annotations"]["readOnlyHint"], true);
        assert!(!ChatToolScope::PersonalDataOnly.allows(GET_CONTROL_DEVICE_ACTION_TOOL_NAME));

        let full_initialize = initialize_response(json!(2), None, ChatToolScope::Full);
        let full_instructions = full_initialize["result"]["instructions"].as_str().unwrap();
        assert!(full_instructions.contains(LIST_CONTROL_DEVICES_TOOL_NAME));
        assert!(full_instructions.contains(CONTROL_DEVICE_TOOL_NAME));
        assert!(full_instructions.contains(GET_CONTROL_DEVICE_ACTION_TOOL_NAME));
        assert!(full_instructions.contains("commande exacte"));
        let personal_initialize =
            initialize_response(json!(3), None, ChatToolScope::PersonalDataOnly);
        let personal_instructions = personal_initialize["result"]["instructions"]
            .as_str()
            .unwrap();
        assert!(!personal_instructions.contains(LIST_CONTROL_DEVICES_TOOL_NAME));
        assert!(!personal_instructions.contains(CONTROL_DEVICE_TOOL_NAME));
        assert!(!personal_instructions.contains(GET_CONTROL_DEVICE_ACTION_TOOL_NAME));
    }

    #[test]
    fn microsoft_tools_never_expose_a_mailbox_selector() {
        let response = tools_list_response(json!(1), ChatToolScope::Full);
        let tools = response["result"]["tools"].as_array().unwrap();
        for name in [
            LIST_OUTLOOK_MESSAGES_TOOL_NAME,
            LIST_CALENDAR_EVENTS_TOOL_NAME,
            SEND_OUTLOOK_EMAIL_TOOL_NAME,
            CREATE_CALENDAR_EVENT_TOOL_NAME,
            UPDATE_CALENDAR_EVENT_TOOL_NAME,
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("outil {name} absent de tools/list"));
            let properties = &tool["inputSchema"]["properties"];
            // Aucun champ ne doit permettre de viser l'IDENTITE d'un autre
            // utilisateur de l'application. `account` fait exception : il ne
            // choisit qu'entre les boites deja liees par CET utilisateur, et une
            // adresse inconnue est refusee cote serveur.
            for forbidden in ["userId", "accountId", "mailbox", "from", "sender", "owner"] {
                assert!(
                    properties.get(forbidden).is_none(),
                    "{name} expose {forbidden}"
                );
            }
            // Le selecteur de boite existe, est optionnel, et cible les propres
            // adresses de l'utilisateur.
            assert!(
                properties.get("account").is_some(),
                "{name} n'expose pas le selecteur de boite"
            );
            let required = tool["inputSchema"]["required"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !required.iter().any(|value| value == "account"),
                "{name} rend le selecteur de boite obligatoire"
            );
            assert_eq!(tool["inputSchema"]["additionalProperties"], false);
            // Ces outils sortent de l'application : annoncer un monde ferme au
            // client MCP masquerait le risque reel.
            assert_eq!(tool["annotations"]["openWorldHint"], true);
        }

        for name in [
            SEND_OUTLOOK_EMAIL_TOOL_NAME,
            CREATE_CALENDAR_EVENT_TOOL_NAME,
            UPDATE_CALENDAR_EVENT_TOOL_NAME,
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool["name"].as_str() == Some(name))
                .unwrap();
            let description = tool["description"].as_str().unwrap();
            assert!(
                description.contains("N'ENVOIE RIEN")
                    || description.contains("NE CREE RIEN")
                    || description.contains("NE MODIFIE RIEN"),
                "{name} ne previent pas le modele que rien ne part sans confirmation"
            );
        }

        // Les pieces jointes sont volontairement absentes : le corps MCP est
        // plafonne a 64 Kio et un fichier encode en base64 serait rejete par
        // axum avant meme d'atteindre le dispatch.
        let send = tools
            .iter()
            .find(|tool| tool["name"].as_str() == Some(SEND_OUTLOOK_EMAIL_TOOL_NAME))
            .unwrap();
        assert!(send["inputSchema"]["properties"]
            .get("attachments")
            .is_none());
        assert_eq!(
            send["inputSchema"]["required"],
            json!(["to", "subject", "body"])
        );
    }
}
