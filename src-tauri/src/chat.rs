//! Moteur de conversations sans terminal visible.
//!
//! Chaque message lance le provider en mode non interactif (`codex exec`,
//! `claude --print` ou `opencode run`). Les sessions restent celles des CLI.

use crate::{
    chat_model_tools::{
        ChatModelToolServerConfig, ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME,
        APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME, AUTONOMOUS_AGENT_TOOL_NAME,
        CREATE_CALENDAR_EVENT_TOOL_NAME, CREATE_CHAT_TOOL_NAME, CREATE_GOAL_TOOL_NAME,
        LIST_CALENDAR_EVENTS_TOOL_NAME, LIST_OUTLOOK_MESSAGES_TOOL_NAME,
        LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME, LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME,
        LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME, MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME,
        MCP_BEARER_ENV, MCP_SERVER_NAME, PAUSE_AUTONOMOUS_AGENT_TOOL_NAME,
        PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME, QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME,
        SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME, SEND_CHAT_MESSAGE_TOOL_NAME,
        SEND_OUTLOOK_EMAIL_TOOL_NAME, SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
        UPDATE_AUTONOMOUS_AGENT_TOOL_NAME, UPDATE_CALENDAR_EVENT_TOOL_NAME,
    },
    chat_tools::{chat_skills_document, chat_tool_instructions, ChatAgentSkill, ChatAgentTool},
    discussions::{self, DiscussionContextUsage},
    metrics,
    runtime_sync::{RuntimeSync, RuntimeSyncTopic},
    settings::{self, AccountProfile, AppSettings, Provider},
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use flate2::{read::ZlibDecoder, write::ZlibEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    env,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        mpsc, Arc, Mutex, Once, Weak,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
#[cfg(feature = "desktop")]
use tauri::State;
use uuid::Uuid;

const MAX_PROMPT_BYTES: usize = 256 * 1024;
/// Seuil de journalisation d'une soumission anormalement grosse. En dessous de
/// `MAX_PROMPT_BYTES` (qui, lui, rejette) : c'est un signal, pas un refus.
const OVERSIZED_PROMPT_WARN_BYTES: usize = 32 * 1024;
const MAX_CHAT_IMAGES: usize = 4;
const MAX_CHAT_IMAGE_BYTES: usize = 100 * 1024 * 1024;
const MAX_CHAT_IMAGE_TOTAL_BYTES: usize = 100 * 1024 * 1024;
// Le corps HTTP transporte les images en base64 (~+33 %) : 100 Mo bruts pesent
// ~133 Mo une fois encodes. Le plafond de requete doit donc rester au-dessus,
// avec une marge pour le prompt, les skills et la structure JSON.
pub(crate) const MAX_CHAT_TURN_REQUEST_BYTES: usize = 150 * 1024 * 1024;
const MAX_ERROR_BYTES: usize = 24 * 1024;
const MAX_ACTIVITIES: usize = 32;
const MAX_THOUGHTS: usize = 32;
const MAX_PARTS: usize = 96;
const MAX_THOUGHT_CHARS: usize = 4_000;
const MAX_PART_DETAIL_CHARS: usize = 12_000;
const MAX_MODEL_CHARS: usize = 160;
const MAX_RETAINED_TURNS: usize = 500;
/// `0` desactive le plafond numerique. L'admission se fait alors uniquement sur
/// la marge memoire et sur la limite cgroup du conteneur.
const DEFAULT_MAX_ACTIVE_CHAT_TURNS: usize = 0;
const DEFAULT_MIN_CONTAINER_HEADROOM_MIB: u64 = 1536;
const DEFAULT_MIN_HOST_AVAILABLE_MIB: u64 = 1536;
const PROVIDER_EXIT_GRACE: Duration = Duration::from_secs(2);
const COMPACT_TIMEOUT: Duration = Duration::from_secs(180);
/// Le serveur MCP vit dans le meme processus HTTP que l'API du chat. Sous forte
/// charge, son handshake peut depasser les cinq secondes historiques alors que
/// le serveur est sain. On lui laisse une marge, mais il reste optionnel : une
/// indisponibilite transitoire des outils ne doit jamais bloquer la reprise
/// d'une conversation ni faire perdre le message de l'utilisateur.
const CHAT_MCP_STARTUP_TIMEOUT_SECONDS: u64 = 15;
const OPENCODE_CONFIG_CONTENT_ENV: &str = "OPENCODE_CONFIG_CONTENT";
const RESPONSE_QUALITY_INSTRUCTIONS: &str = "Avant toute réponse finale destinée à l'utilisateur, effectue une relecture silencieuse. Corrige les fautes de grammaire, de syntaxe, d'orthographe, d'accord et de ponctuation, puis vérifie que les phrases sont naturelles et non ambiguës dans la langue de l'utilisateur, sauf demande contraire. Pour le code, les commandes et les formats structurés, préserve les éléments littéraux et vérifie que la syntaxe ainsi que tous les délimiteurs et blocs sont complets. Ne modifie pas les citations ou les contenus demandés mot pour mot et ne mentionne pas cette relecture.";

/// Les identifiants de processus renvoyes par les outils d'execution sont lies
/// au runtime du tour. Apres une reprise ou un redemarrage, l'ancien identifiant
/// peut etre inconnu alors que la conversation elle-meme reste parfaitement
/// reprenable. Le modele doit alors relancer l'operation utile au lieu de boucler
/// sur le handle perime ou d'abandonner le chat.
const CODEX_PROCESS_CONTINUITY_INSTRUCTIONS: &str = "Les identifiants de processus ou de cellule retournés par `exec`, `write_stdin` et `wait` sont temporaires. Si `write_stdin` ou `wait` répond `Unknown process id`, ne réutilise plus cet identifiant : vérifie l'état du résultat attendu, puis relance seulement l'opération encore nécessaire de façon idempotente et continue la tâche. Une erreur isolée d'outil n'est pas une raison de terminer la conversation.";

/// Politique commune aux chats structures. Le chat racine agit comme
/// orchestrateur sans attendre une demande explicite, mais les sous-agents
/// restent bornes et ne recreent pas eux-memes une equipe en cascade.
pub(crate) const DEFAULT_ORCHESTRATOR_INSTRUCTIONS: &str = "Tu es l'agent racine et l'orchestrateur par défaut de cette conversation. Dès qu'une demande contient au moins une sous-tâche indépendante et utile, délègue-la proactivement à un ou plusieurs sous-agents et synthétise leurs résultats dans le chat principal ; n'attends pas que l'utilisateur demande explicitement le multi-agent. Garde localement les questions simples et les opérations atomiques pour éviter un surcoût inutile. Donne à chaque sous-agent une mission bornée, attends les résultats nécessaires, puis vérifie l'intégration. Si tu es toi-même un sous-agent ou un worker, accomplis uniquement ta mission et ne crée pas une nouvelle équipe sauf nécessité explicite.";
const ORCHESTRATION_CHILD_INSTRUCTIONS: &str = "Tu es un membre d'une orchestration deja active. Accomplis uniquement la mission bornee fournie par l'orchestrateur. Ne cree, ne lance et ne delegue aucun sous-agent, aucune nouvelle equipe et aucune orchestration imbriquee. Travaille exclusivement dans le dossier courant confine et rends les preuves structurees demandees.";

/// Classification commune injectee dans chaque tour, quel que soit le provider,
/// afin qu'une reprise de conversation conserve la separation dev/stable.
const SWITCH_ENVIRONMENT_INSTRUCTIONS: &str = "Classification canonique des environnements Switch : toute version stockée sur le SSD/Samsung T7 ou exécutée sur le poste local pc-fixe est exclusivement une version de développement. La version hébergée sur le VPS Microsoft Azure est la seule version stable/production. Sans autre précision, les termes stable, production et prod désignent le VPS Azure, jamais la copie SSD/locale. Garde ces environnements séparés et ne synchronise, ne promeus, ne déploie ou ne redémarre jamais l'un depuis l'autre sans demande explicite de l'utilisateur.";

/// Filet applique aux tours Claude one-shot : chaque tour est un process
/// `claude --print` distinct qui meurt a la fin du tour. Toute commande lancee
/// en arriere-plan (`run_in_background`) est donc orpheline et resurgit au tour
/// suivant sous forme de `<task-notification>` "no completion record", que le
/// modele traite a la place de la demande. Tant que la session persistante
/// n'est pas active pour ce tour, on interdit l'arriere-plan.
const CLAUDE_FOREGROUND_SHELL_INSTRUCTIONS: &str = "N'exécute jamais de commande shell en arrière-plan : n'utilise pas l'option d'exécution en tâche de fond (run_in_background). Lance chaque commande au premier plan et attends sa fin avant de continuer. Pour une commande longue, augmente son délai d'attente au lieu de la détacher.";

/// Duree d'inactivite au-dela de laquelle une session Claude persistante est
/// recyclee (le process est arrete pour liberer la memoire du noeud). Volontai-
/// rement genereux : une conversation active reste chaude entre deux messages.
const DEFAULT_LIVE_CLAUDE_IDLE_SECONDS: usize = 3 * 60;

/// `0` conserve toutes les sessions Claude tant que la marge memoire suffit.
/// Le balayage d'inactivite continue a recycler les sessions froides.
const DEFAULT_LIVE_CLAUDE_MAX_SESSIONS: usize = 0;

/// Delai d'attente laisse au process pour finaliser un tour apres une demande
/// d'interruption (control_request `interrupt`) avant de basculer sur l'arret
/// complet du process. En pratique l'avortement emet un `result` en une fraction
/// de seconde ; la marge couvre un noeud charge. Au-dela, interruption echouee.
const LIVE_CLAUDE_INTERRUPT_DRAIN: Duration = Duration::from_secs(3);

fn positive_env_mib(name: &str, fallback_mib: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback_mib)
}

fn positive_env_usize(name: &str, fallback: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(fallback)
}

pub(crate) fn configured_max_active_chat_turns() -> usize {
    env::var("CST_CHAT_MAX_ACTIVE")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_ACTIVE_CHAT_TURNS)
}

fn live_claude_idle_timeout() -> Duration {
    Duration::from_secs(positive_env_usize(
        "CST_CLAUDE_IDLE_SECONDS",
        DEFAULT_LIVE_CLAUDE_IDLE_SECONDS,
    ) as u64)
}

fn live_claude_max_sessions() -> usize {
    env::var("CST_CLAUDE_MAX_SESSIONS")
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_LIVE_CLAUDE_MAX_SESSIONS)
}

fn cgroup_container_headroom_bytes() -> Option<u64> {
    let maximum = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    if maximum.trim() == "max" {
        return None;
    }
    let maximum = maximum.trim().parse::<u64>().ok()?;
    let current = std::fs::read_to_string("/sys/fs/cgroup/memory.current")
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(maximum.saturating_sub(current))
}

fn host_available_memory_bytes() -> Option<u64> {
    std::fs::read_to_string("/proc/meminfo")
        .ok()?
        .lines()
        .find_map(|line| {
            let value = line.strip_prefix("MemAvailable:")?;
            value
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kib| kib.saturating_mul(1024))
        })
}

fn enforce_chat_resource_admission() -> Result<(), String> {
    let container_required = positive_env_mib(
        "CST_CHAT_MIN_CONTAINER_HEADROOM_MIB",
        DEFAULT_MIN_CONTAINER_HEADROOM_MIB,
    )
    .saturating_mul(1024 * 1024);
    if cgroup_container_headroom_bytes().is_some_and(|value| value < container_required) {
        return Err("memoire insuffisante: le nouveau chat est temporairement refuse pour proteger Switch et Duello".to_string());
    }

    let host_required = positive_env_mib(
        "CST_CHAT_MIN_HOST_AVAILABLE_MIB",
        DEFAULT_MIN_HOST_AVAILABLE_MIB,
    )
    .saturating_mul(1024 * 1024);
    if host_available_memory_bytes().is_some_and(|value| value < host_required) {
        return Err("memoire insuffisante sur l'hote: le nouveau chat est temporairement refuse pour proteger Switch et Duello".to_string());
    }

    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatTurnMode {
    #[default]
    Build,
    Plan,
    Ask,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum ChatFilesystemScope {
    #[default]
    Default,
    /// Conserve le projet source en lecture seule pendant une review humaine,
    /// mais autorise les captures temporaires sous `.codex-proof/`.
    ReviewProofArtifacts,
    /// Worktree prive cree et controle par `OrchestrationManager`. Le compte
    /// ne peut jamais reutiliser ici son bypass global ni le multi-agent.
    OrchestrationWorkspace,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatTurnStatus {
    Running,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatActivity {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub detail: Option<String>,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatThought {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub status: String,
}

/// Element ordonne d'un tour, sur le modele de la timeline OpenCode.
///
/// `reasoning`, `text` et `tool` partagent volontairement une seule liste :
/// l'interface peut ainsi conserver l'ordre exact dans lequel le provider a
/// raisonne, explique sa progression, appele un outil puis repris sa reponse.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatPart {
    pub id: String,
    pub kind: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnSnapshot {
    pub id: u64,
    pub account_id: String,
    pub session_id: Option<String>,
    /// Cle du chat (pane) a l'origine du tour. Permet au client de reconcilier un
    /// tour serveur deja lance meme quand la reponse HTTP de `start_chat_turn`
    /// s'est perdue et que le tour n'a pas encore de `session_id`.
    #[serde(default)]
    pub source_chat_key: Option<String>,
    pub status: ChatTurnStatus,
    pub started_at: i64,
    pub finished_at: Option<i64>,
    pub error: Option<String>,
    pub activities: Vec<ChatActivity>,
    pub thoughts: Vec<ChatThought>,
    pub parts: Vec<ChatPart>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactChatSessionResult {
    pub context_usage: Option<DiscussionContextUsage>,
}

/// Etat leger destine au polling global : la timeline complete reste sur
/// `chat_turn_status` afin de ne pas multiplier le trafic avec plusieurs chats.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveChatTurnSummary {
    pub id: u64,
    pub account_id: String,
    pub session_id: Option<String>,
    pub source_chat_key: Option<String>,
    pub status: ChatTurnStatus,
    pub started_at: i64,
    pub waiting_for_user: bool,
    pub project_dir: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChatAppConnector {
    Gmail,
    GoogleCalendar,
}

impl ChatAppConnector {
    fn config_id(self) -> &'static str {
        match self {
            Self::Gmail => "gmail",
            Self::GoogleCalendar => "connector_googlecalendar",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatImageAttachmentRequest {
    pub name: String,
    pub mime_type: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartChatTurnRequest {
    pub account_id: String,
    #[serde(default)]
    pub session_id: Option<String>,
    pub prompt: String,
    #[serde(default)]
    pub image_attachments: Vec<ChatImageAttachmentRequest>,
    #[serde(default)]
    pub project_dir: Option<String>,
    #[serde(default)]
    pub mode: ChatTurnMode,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    /// `None` preserve la configuration habituelle d'un chat. `Some`, meme
    /// vide, borne explicitement les apps exposees a un tour autonome.
    #[serde(default)]
    pub app_connectors: Option<Vec<ChatAppConnector>>,
    #[serde(default)]
    pub app_write_approved: bool,
    #[serde(default)]
    pub agent_tools: Vec<ChatAgentTool>,
    #[serde(default)]
    pub agent_skills: Vec<ChatAgentSkill>,
    #[serde(default)]
    pub question_tool: bool,
    #[serde(default)]
    pub proof_tool: bool,
    #[serde(default)]
    pub source_chat_key: Option<String>,
}

struct ChatTurn {
    snapshot: Mutex<ChatTurnSnapshot>,
    archived_snapshot: Mutex<Option<Vec<u8>>>,
    output_closed: AtomicBool,
    child: Mutex<Option<Child>>,
    provider_terminal: Mutex<Option<ProviderTerminalEvent>>,
    runtime_sync: RuntimeSync,
    /// Session Claude persistante qui pilote ce tour, le cas echeant. Vide pour
    /// les tours one-shot (le process vit alors dans `child`). Reference faible :
    /// un tour termine et conserve dans l'historique ne doit pas empecher le
    /// recyclage de la session.
    live_session: Weak<LiveClaudeSession>,
}

struct TemporaryChatSkillsFile {
    path: PathBuf,
}

impl TemporaryChatSkillsFile {
    fn create(turn_id: u64, content: &str) -> Result<Self, String> {
        let directory = env::temp_dir()
            .join("codex-switch-terminal")
            .join("chat-skills");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Préparation des skills impossible : {error}"))?;
        let path = directory.join(format!("turn-{turn_id}-{}.md", Uuid::new_v4()));
        std::fs::write(&path, content)
            .map_err(|error| format!("Préparation des skills impossible : {error}"))?;
        Ok(Self { path })
    }

    fn instructions(&self) -> String {
        format!(
            "Skills explicitement activés par l'utilisateur pour ce tour. Avant toute action, lis intégralement le fichier UTF-8 `{}` avec les outils disponibles, puis applique toutes ses sections en plus de la demande utilisateur. Ne modifie pas ce fichier et ne l'affiche pas dans la réponse.",
            self.path.display()
        )
    }
}

impl Drop for TemporaryChatSkillsFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Repertoire des images de chat temporaires. Chaque image y vit le temps d'un
/// tour (supprimee par `Drop` en fin de tour) ; le balayeur y efface aussi les
/// orphelines laissees par un tour qui a plante avant son nettoyage.
fn chat_images_dir() -> PathBuf {
    env::temp_dir()
        .join("codex-switch-terminal")
        .join("chat-images")
}

struct TemporaryChatImages {
    paths: Vec<PathBuf>,
}

impl TemporaryChatImages {
    fn create(turn_id: u64, attachments: &[ChatImageAttachmentRequest]) -> Result<Self, String> {
        if attachments.len() > MAX_CHAT_IMAGES {
            return Err(format!(
                "Un message peut contenir au maximum {MAX_CHAT_IMAGES} images"
            ));
        }
        let directory = chat_images_dir();
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Preparation des images impossible : {error}"))?;
        let mut images = Self { paths: Vec::new() };
        let mut total_bytes = 0_usize;

        for (index, attachment) in attachments.iter().enumerate() {
            let label = attachment.name.trim();
            if label.chars().count() > 160 || label.chars().any(char::is_control) {
                return Err(format!("Nom de l'image {} invalide", index + 1));
            }
            let encoded = attachment.data_base64.trim();
            let max_encoded_bytes = ((MAX_CHAT_IMAGE_BYTES + 2) / 3) * 4;
            if encoded.is_empty() || encoded.len() > max_encoded_bytes {
                return Err(format!("L'image {} depasse 100 Mo", index + 1));
            }
            let bytes = BASE64_STANDARD
                .decode(encoded)
                .map_err(|_| format!("Donnees de l'image {} invalides", index + 1))?;
            if bytes.is_empty() || bytes.len() > MAX_CHAT_IMAGE_BYTES {
                return Err(format!("L'image {} depasse 100 Mo", index + 1));
            }
            total_bytes = total_bytes.saturating_add(bytes.len());
            if total_bytes > MAX_CHAT_IMAGE_TOTAL_BYTES {
                return Err("Les images d'un message depassent 100 Mo au total".to_string());
            }
            let extension = validated_chat_image_extension(&attachment.mime_type, &bytes)
                .ok_or_else(|| format!("Format de l'image {} invalide", index + 1))?;
            let path = directory.join(format!(
                "turn-{turn_id}-{}-{index}.{extension}",
                Uuid::new_v4()
            ));
            std::fs::write(&path, bytes)
                .map_err(|error| format!("Enregistrement de l'image impossible : {error}"))?;
            images.paths.push(path);
        }
        Ok(images)
    }

    fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    fn instructions(&self) -> Option<String> {
        if self.paths.is_empty() {
            return None;
        }
        let paths = self
            .paths
            .iter()
            .map(|path| format!("- `{}`", path.display()))
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "L'utilisateur a joint une ou plusieurs images a ce tour. Lis chaque fichier avec l'outil de lecture d'images avant de repondre :\n{paths}"
        ))
    }
}

impl Drop for TemporaryChatImages {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Efface les images de chat orphelines : fichiers qu'un tour n'a pas pu nettoyer
/// (process tue, crash) et qui resteraient sinon indefiniment sur le disque.
/// `max_age` borne la suppression afin de ne jamais toucher l'image d'un tour
/// encore en cours (dont le fichier vient tout juste d'etre ecrit).
fn sweep_orphan_chat_images_in(directory: &Path, max_age: Duration) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return; // Dossier absent : rien a nettoyer.
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let expired = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .map(|age| age >= max_age)
            .unwrap_or(false); // Age illisible : on ne supprime pas par prudence.
        if expired {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// Demarre le nettoyage automatique des images orphelines : un balayage immediat
/// au demarrage (tout residu vient forcement d'un run precedent), puis un passage
/// horaire. Le seuil d'age genereux garantit qu'un tour en cours n'est jamais
/// touche. Idempotent : un seul balayeur par processus.
pub(crate) fn start_orphan_chat_image_sweeper() {
    static STARTED: Once = Once::new();
    STARTED.call_once(|| {
        let directory = chat_images_dir();
        // Au demarrage, aucun tour de ce processus n'a encore ecrit d'image :
        // tout fichier de plus de 2 min provient d'un run precedent.
        sweep_orphan_chat_images_in(&directory, Duration::from_secs(2 * 60));
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(60 * 60));
            // Un tour ne conserve jamais une image 6 h : au-dela, c'est un orphelin.
            sweep_orphan_chat_images_in(&directory, Duration::from_secs(6 * 60 * 60));
        });
    });
}

fn validated_chat_image_extension(mime_type: &str, bytes: &[u8]) -> Option<&'static str> {
    match mime_type.trim().to_ascii_lowercase().as_str() {
        "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => Some("png"),
        "image/jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => Some("jpg"),
        "image/webp"
            if bytes.len() >= 12
                && bytes.starts_with(b"RIFF")
                && bytes.get(8..12) == Some(b"WEBP") =>
        {
            Some("webp")
        }
        _ => None,
    }
}

struct TemporaryModelToolConfigFile {
    path: PathBuf,
}

impl TemporaryModelToolConfigFile {
    fn create(turn_id: u64, config: &ChatModelToolServerConfig) -> Result<Self, String> {
        let directory = env::temp_dir()
            .join("codex-switch-terminal")
            .join("chat-mcp");
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("Preparation de l'outil autonome impossible : {error}"))?;
        let path = directory.join(format!("turn-{turn_id}-{}.json", Uuid::new_v4()));
        let document = json!({
            "mcpServers": {
                (MCP_SERVER_NAME): {
                    "type": "http",
                    "url": config.url.clone(),
                    "headers": {
                        "Authorization": format!("Bearer {}", config.bearer_token)
                    }
                }
            }
        });
        let content = serde_json::to_vec(&document)
            .map_err(|error| format!("Configuration de l'outil autonome invalide : {error}"))?;
        std::fs::write(&path, content)
            .map_err(|error| format!("Preparation de l'outil autonome impossible : {error}"))?;
        Ok(Self { path })
    }
}

impl Drop for TemporaryModelToolConfigFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Clone)]
enum ProviderTerminalOutcome {
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
struct ProviderTerminalEvent {
    outcome: ProviderTerminalOutcome,
    observed_at: Instant,
}

impl Drop for ChatTurn {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            if let Some(child) = child.as_mut() {
                let _ = terminate_chat_process_tree(child);
            }
        }
    }
}

#[derive(Clone)]
pub struct ChatTurnManager {
    turns: Arc<Mutex<HashMap<u64, Arc<ChatTurn>>>>,
    /// Proprietaire SaaS de chaque tour. Absent pour les executions desktop ou
    /// administratives internes, qui ne sont jamais exposees a un autre compte.
    owners: Arc<Mutex<HashMap<u64, String>>>,
    /// Environnement d'execution de chaque tour. C'est la seule identite que
    /// partagent tous les appareils : un tour lance au jeton administrateur, par
    /// un agent autonome ou par une orchestration n'a aucun proprietaire
    /// nominatif, et resterait donc invisible — donc « Disponible » — dans les
    /// sessions web et mobiles alors qu'il tourne.
    workspaces: Arc<Mutex<HashMap<u64, String>>>,
    claims: Arc<Mutex<HashSet<String>>>,
    /// Serialise la mesure de marge et le lancement du fournisseur afin que
    /// deux requetes ne consomment pas simultanement la meme marge memoire.
    start_gate: Arc<Mutex<()>>,
    max_active: usize,
    /// Sessions Claude persistantes vivantes, indexees par `account_id\0session_id`.
    /// Un seul process par conversation ; les taches shell en arriere-plan y
    /// survivent d'un tour a l'autre.
    live_claude: Arc<Mutex<HashMap<String, Arc<LiveClaudeSession>>>>,
    next_id: Arc<AtomicU64>,
    runtime_sync: RuntimeSync,
}

impl Default for ChatTurnManager {
    fn default() -> Self {
        Self::with_max_active(configured_max_active_chat_turns())
    }
}

impl ChatTurnManager {
    pub(crate) fn with_max_active(max_active: usize) -> Self {
        Self {
            turns: Arc::new(Mutex::new(HashMap::new())),
            owners: Arc::new(Mutex::new(HashMap::new())),
            workspaces: Arc::new(Mutex::new(HashMap::new())),
            claims: Arc::new(Mutex::new(HashSet::new())),
            start_gate: Arc::new(Mutex::new(())),
            max_active,
            live_claude: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(0)),
            runtime_sync: RuntimeSync::default(),
        }
    }
}

struct ChatClaim {
    claims: Arc<Mutex<HashSet<String>>>,
    key: Option<String>,
}

impl ChatTurnManager {
    pub(crate) fn runtime_sync(&self) -> RuntimeSync {
        self.runtime_sync.clone()
    }

    pub(crate) fn notify_autonomous_agents_changed(&self) {
        self.runtime_sync.notify(RuntimeSyncTopic::AutonomousAgents);
    }

    pub(crate) fn assign_owner(&self, id: u64, owner_id: &str) -> Result<(), String> {
        if !self
            .turns
            .lock()
            .map_err(|_| "Etat des conversations verrouille".to_string())?
            .contains_key(&id)
        {
            return Err("Tour de conversation introuvable".to_string());
        }
        self.owners
            .lock()
            .map_err(|_| "Proprietaires des conversations verrouilles".to_string())?
            .insert(id, owner_id.to_string());
        Ok(())
    }

    pub(crate) fn is_owned_by(&self, id: u64, owner_id: &str) -> Result<bool, String> {
        Ok(self
            .owners
            .lock()
            .map_err(|_| "Proprietaires des conversations verrouilles".to_string())?
            .get(&id)
            .is_some_and(|owner| owner == owner_id))
    }

    /// Retient l'environnement dans lequel tourne un tour. Il complete le
    /// proprietaire nominatif, absent des lancements administrateurs, autonomes
    /// et orchestres.
    fn remember_turn_workspace(&self, id: u64, project_dir: Option<&Path>) {
        let Some(path) = project_dir else {
            return;
        };
        // Le chemin complet, pas le libelle de `display_path` : c'est
        // `authorize_existing_environment` qui le relira, et il canonicalise.
        if let Ok(mut workspaces) = self.workspaces.lock() {
            workspaces.insert(id, path.to_string_lossy().to_string());
        }
    }

    fn turn_workspace(&self, id: u64) -> Result<Option<String>, String> {
        Ok(self
            .workspaces
            .lock()
            .map_err(|_| "Environnements des conversations verrouilles".to_string())?
            .get(&id)
            .cloned())
    }

    /// Un tour est visible par un utilisateur nominatif lorsqu'il l'a lance
    /// lui-meme, ou lorsqu'il tourne dans un environnement auquel il a acces.
    ///
    /// Le second critere est ce qui aligne le statut entre tous ses appareils :
    /// sans lui, un chat lance depuis l'application de bureau (jeton
    /// administrateur), un agent autonome ou une orchestration s'affiche
    /// « En cours » la ou il a demarre et « Disponible » partout ailleurs.
    pub(crate) fn is_visible_to(
        &self,
        id: u64,
        owner_id: &str,
        workspace_is_authorized: impl FnOnce(&str) -> bool,
    ) -> Result<bool, String> {
        if self.is_owned_by(id, owner_id)? {
            return Ok(true);
        }
        Ok(self
            .turn_workspace(id)?
            .is_some_and(|path| workspace_is_authorized(&path)))
    }

    pub(crate) fn active_visible_to(
        &self,
        owner_id: &str,
        mut workspace_is_authorized: impl FnMut(&str) -> bool,
    ) -> Result<Vec<ActiveChatTurnSummary>, String> {
        // Les deux index sont copies avant le filtrage : l'autorisation touche
        // le disque et ne doit jamais s'executer sous un verrou du catalogue.
        let active = self.active()?;
        let owners = self
            .owners
            .lock()
            .map_err(|_| "Proprietaires des conversations verrouilles".to_string())?
            .clone();
        let workspaces = self
            .workspaces
            .lock()
            .map_err(|_| "Environnements des conversations verrouilles".to_string())?
            .clone();
        Ok(active
            .into_iter()
            .filter(|snapshot| {
                owners
                    .get(&snapshot.id)
                    .is_some_and(|owner| owner == owner_id)
                    || workspaces
                        .get(&snapshot.id)
                        .is_some_and(|path| workspace_is_authorized(path))
            })
            .collect())
    }

    pub fn start(&self, request: StartChatTurnRequest) -> Result<ChatTurnSnapshot, String> {
        self.start_with_model_tools(request, None)
    }

    pub(crate) fn start_with_model_tools(
        &self,
        request: StartChatTurnRequest,
        model_tool_server: Option<ChatModelToolServerConfig>,
    ) -> Result<ChatTurnSnapshot, String> {
        self.start_with_scope(request, model_tool_server, ChatFilesystemScope::Default)
    }

    pub(crate) fn start_review_planning(
        &self,
        request: StartChatTurnRequest,
    ) -> Result<ChatTurnSnapshot, String> {
        self.start_with_scope(request, None, ChatFilesystemScope::ReviewProofArtifacts)
    }

    pub(crate) fn start_orchestration(
        &self,
        request: StartChatTurnRequest,
    ) -> Result<ChatTurnSnapshot, String> {
        self.start_with_scope(request, None, ChatFilesystemScope::OrchestrationWorkspace)
    }

    fn start_with_scope(
        &self,
        request: StartChatTurnRequest,
        model_tool_server: Option<ChatModelToolServerConfig>,
        filesystem_scope: ChatFilesystemScope,
    ) -> Result<ChatTurnSnapshot, String> {
        self.prune_finished_turns();
        let _start_guard = self
            .start_gate
            .lock()
            .map_err(|_| "Admission des chats verrouillee".to_string())?;
        let active = self.active_count();
        if self.max_active > 0 && active >= self.max_active {
            return Err(format!(
                "capacite chats atteinte: {active}/{} tours actifs; le message doit attendre qu'une place se libere",
                self.max_active
            ));
        }
        enforce_chat_resource_admission()?;
        let prompt = request.prompt.trim().to_string();
        if prompt.is_empty() && request.image_attachments.is_empty() {
            return Err("Le message est vide".to_string());
        }
        let prompt = if prompt.is_empty() {
            "Image jointe.".to_string()
        } else {
            prompt
        };
        if prompt.len() > MAX_PROMPT_BYTES {
            return Err("Le message est trop volumineux".to_string());
        }
        // Tout ce que l'application compose elle-meme (amorce de reprise, relais
        // d'orchestration) est borne en amont bien en dessous de ce seuil. Une
        // soumission qui l'atteint quand meme est donc anormale : on la trace,
        // sinon un prochain incident sera aussi difficile a attribuer que
        // l'injection de competence affichee comme un message utilisateur.
        if prompt.len() > OVERSIZED_PROMPT_WARN_BYTES {
            eprintln!(
                "[chat] prompt volumineux: {} octets (compte {}, chat {})",
                prompt.len(),
                request.account_id,
                request.source_chat_key.as_deref().unwrap_or("-"),
            );
        }
        let app_settings = settings::load_settings_for_terminal()?;
        let account = app_settings
            .accounts
            .iter()
            .find(|candidate| candidate.id == request.account_id)
            .cloned()
            .ok_or_else(|| "Compte introuvable".to_string())?;
        if account.provider == Provider::Freebuff {
            return Err(format!(
                "Le chat ne prend pas en charge freebuff ({}) : son CLI est un TUI interactif, sans prompt en argument ni sortie structuree. Ouvre-le dans un onglet terminal.",
                account.label
            ));
        }
        if let Some(session_id) = request.session_id.as_deref() {
            validate_session_id(account.provider, session_id)?;
        }
        if !settings::account_has_auth_tokens(&account) {
            return Err(format!(
                "Compte non authentifie : {}. Ouvre un terminal de connexion pour ce compte avant de lancer un chat.",
                account.label
            ));
        }
        let model = selected_model(request.model.as_deref(), account.model.as_deref())?;
        let reasoning_effort = selected_reasoning_effort(
            account.provider,
            account.inference_provider.as_deref(),
            request.reasoning_effort.as_deref(),
            account.reasoning_effort.as_deref(),
        )?;

        // Phase 1 de la session Claude persistante : volontairement restreinte
        // aux tours "simples" ou aucun outil interactif ne peut declencher une
        // demande d'approbation (`can_use_tool`) qui figerait le flux stream-json.
        // - `session_id` connu => tour 2+, on peut `--resume` dans le process vif ;
        // - mode Build en bypass => aucune approbation d'outil demandee ;
        // - pas d'images / skills / MCP autonome => aucune instruction ni fichier
        //   temporaire propre au tour a injecter apres le lancement du process.
        // Tout le reste retombe sur le chemin one-shot habituel, protege par le
        // filet anti-arriere-plan ci-dessous.
        let use_persistent_claude = app_settings.claude_persistent_session
            && account.provider == Provider::Claude
            && request.session_id.is_some()
            && request.mode == ChatTurnMode::Build
            && account.bypass
            && request.image_attachments.is_empty()
            && request.agent_skills.is_empty()
            && model_tool_server.is_none()
            && filesystem_scope == ChatFilesystemScope::Default;
        // Filet (Option A) : hors session persistante, un tour Claude one-shot ne
        // doit pas lancer de tache en arriere-plan, sous peine de l'orpheliner.
        let apply_foreground_net = account.provider == Provider::Claude && !use_persistent_claude;

        let claim = self.reserve_turn(&request)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed) + 1;
        let image_files = TemporaryChatImages::create(id, &request.image_attachments)?;

        let canonical_home = settings::expand_home(&account.codex_home)?;
        std::fs::create_dir_all(&canonical_home).map_err(|error| error.to_string())?;
        let project_dir = resolve_project_dir(&account, request.project_dir.as_deref())?;
        let proof_workspace =
            review_proof_workspace(account.provider, project_dir.as_deref(), filesystem_scope)?;
        let execution_dir = proof_workspace.as_deref().or(project_dir.as_deref());
        let effective_filesystem_scope = match filesystem_scope {
            ChatFilesystemScope::ReviewProofArtifacts if proof_workspace.is_none() => {
                ChatFilesystemScope::Default
            }
            scope => scope,
        };
        account
            .provider
            .write_account_config(
                &canonical_home,
                account.bypass,
                account.model.as_deref(),
                account.reasoning_effort.as_deref(),
                account.fast_mode,
            )
            .map_err(|error| format!("Configuration du compte impossible : {error}"))?;
        if account.provider == Provider::Codex {
            if let Some(session_id) = request.session_id.as_deref() {
                let repaired =
                    discussions::repair_interrupted_codex_tool_calls(&canonical_home, session_id)?;
                if repaired > 0 {
                    eprintln!(
                        "[chat] session Codex {session_id} reparee avant reprise: {repaired} sortie(s) d'outil interrompue(s) completee(s)"
                    );
                }
            }
        }

        let environment_instructions = project_dir
            .as_deref()
            .and_then(|path| settings::workspace_memory_for_path(&app_settings, path))
            .map(environment_memory_instructions);
        let image_instructions = if account.provider == Provider::Codex {
            None
        } else {
            image_files.instructions()
        };
        let tool_instructions = chat_tool_instructions(
            &request.agent_tools,
            request.question_tool,
            request.proof_tool,
        );
        let skill_file = chat_skills_document(&request.agent_skills)?
            .map(|document| TemporaryChatSkillsFile::create(id, &document))
            .transpose()?;
        let skill_instructions = skill_file
            .as_ref()
            .map(TemporaryChatSkillsFile::instructions);
        let model_tool_file = if account.provider == Provider::Claude {
            model_tool_server
                .as_ref()
                .map(|config| TemporaryModelToolConfigFile::create(id, config))
                .transpose()?
        } else {
            None
        };
        let selected_agent_instructions =
            merge_turn_instructions(tool_instructions.as_deref(), skill_instructions.as_deref());
        let model_tool_instructions = model_tool_server
            .as_ref()
            .map(|_| autonomous_agent_tool_instructions());
        let agent_instructions = merge_turn_instructions(
            model_tool_instructions,
            selected_agent_instructions.as_deref(),
        );
        let base_instructions = merge_turn_instructions(
            Some(RESPONSE_QUALITY_INSTRUCTIONS),
            (account.provider == Provider::Codex).then_some(CODEX_PROCESS_CONTINUITY_INSTRUCTIONS),
        );
        let base_instructions = merge_turn_instructions(
            base_instructions.as_deref(),
            Some(SWITCH_ENVIRONMENT_INSTRUCTIONS),
        );
        let base_instructions = merge_turn_instructions(
            base_instructions.as_deref(),
            Some(
                if effective_filesystem_scope == ChatFilesystemScope::OrchestrationWorkspace {
                    ORCHESTRATION_CHILD_INSTRUCTIONS
                } else {
                    DEFAULT_ORCHESTRATOR_INSTRUCTIONS
                },
            ),
        );
        let base_instructions = merge_turn_instructions(
            base_instructions.as_deref(),
            apply_foreground_net.then_some(CLAUDE_FOREGROUND_SHELL_INSTRUCTIONS),
        );
        let turn_instructions = merge_turn_instructions(
            base_instructions.as_deref(),
            environment_instructions.as_deref(),
        );
        let turn_instructions =
            merge_turn_instructions(turn_instructions.as_deref(), agent_instructions.as_deref());
        let turn_instructions =
            merge_turn_instructions(turn_instructions.as_deref(), image_instructions.as_deref());
        let proof_workspace_instructions = proof_workspace.as_deref().and_then(|proof_dir| {
            project_dir
                .as_deref()
                .map(|project_dir| review_proof_workspace_instructions(project_dir, proof_dir))
        });
        let turn_instructions = merge_turn_instructions(
            turn_instructions.as_deref(),
            proof_workspace_instructions.as_deref(),
        );
        let provider_instructions = turn_instructions.as_deref().map(|instructions| {
            if account.provider == Provider::Codex {
                merge_codex_developer_instructions(&canonical_home, instructions)
            } else {
                instructions.to_string()
            }
        });

        // Configure une commande fournisseur neuve et complete. Reutilisee pour
        // le lancement one-shot et, pour Claude, pour (re)demarrer le process
        // persistant : un repli sur one-shot doit pouvoir la reconstruire.
        let build_provider_command = || -> Result<Command, String> {
            let mut command = resolved_provider_command(
                &settings::command_for_provider(&app_settings, account.provider),
                account.provider,
            )?;
            configure_environment(
                &mut command,
                &app_settings,
                &account,
                &canonical_home,
                execution_dir,
            );
            configure_provider_command_with_images_and_scope(
                &mut command,
                &account,
                request.session_id.as_deref(),
                request.mode,
                model.as_deref(),
                reasoning_effort.as_deref(),
                request.app_connectors.as_deref(),
                request.app_write_approved,
                provider_instructions.as_deref(),
                model_tool_server.as_ref(),
                model_tool_file.as_ref().map(|file| file.path.as_path()),
                image_files.paths(),
                effective_filesystem_scope,
            );
            // `configure_provider_command...` pose d'abord le MCP goal dans
            // OPENCODE_CONFIG_CONTENT. L'overlay modele doit donc etre fusionne
            // ensuite, jamais remplacer cette configuration existante.
            configure_opencode_model_overlay(
                &mut command,
                &account,
                model.as_deref(),
                reasoning_effort.as_deref(),
            )?;
            Ok(command)
        };

        // Session Claude persistante : tente de router le tour vers un process
        // `claude` deja vivant (ou d'en demarrer un) plutot que de spawn/kill un
        // process par tour. En cas d'echec, on retombe proprement sur le
        // one-shot ci-dessous sans avoir consomme la reservation ni cree de tour.
        if use_persistent_claude {
            if let Some(session_id) = request.session_id.clone() {
                match build_provider_command() {
                    Ok(mut persistent_command) => {
                        persistent_command
                            .arg("--input-format")
                            .arg("stream-json")
                            .stdin(Stdio::piped())
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());
                        hide_process_window(&mut persistent_command);
                        let profile = live_claude_profile(
                            &canonical_home,
                            execution_dir,
                            request.mode,
                            model.as_deref(),
                            reasoning_effort.as_deref(),
                            provider_instructions.as_deref(),
                        );
                        let detail = project_dir.as_ref().map(|path| display_path(path));
                        match self.start_persistent_claude_turn(
                            persistent_command,
                            &account,
                            &session_id,
                            &profile,
                            id,
                            detail,
                            &prompt,
                            request.source_chat_key.clone(),
                        ) {
                            Ok(snapshot) => {
                                self.remember_turn_workspace(id, project_dir.as_deref());
                                claim.commit();
                                self.runtime_sync.notify(RuntimeSyncTopic::ActiveChatTurns);
                                return Ok(snapshot);
                            }
                            Err(error) => {
                                eprintln!(
                                    "[chat] session Claude persistante indisponible, repli one-shot : {error}"
                                );
                            }
                        }
                    }
                    Err(error) => {
                        eprintln!(
                            "[chat] preparation de la session Claude persistante impossible, repli one-shot : {error}"
                        );
                    }
                }
            }
        }

        let mut command = build_provider_command()?;
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_process_window(&mut command);

        let snapshot = seed_turn_snapshot(
            id,
            &account,
            request.session_id.clone(),
            project_dir.as_ref().map(|path| display_path(path)),
            request.source_chat_key.clone(),
        );
        let turn = Arc::new(ChatTurn {
            snapshot: Mutex::new(snapshot.clone()),
            archived_snapshot: Mutex::new(None),
            output_closed: AtomicBool::new(false),
            child: Mutex::new(None),
            provider_terminal: Mutex::new(None),
            runtime_sync: self.runtime_sync.clone(),
            live_session: Weak::new(),
        });

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(error) => {
                return Err(format!(
                    "Impossible de lancer {} : {error}",
                    provider_label(account.provider)
                ));
            }
        };
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut stdin = child.stdin.take();
        *turn
            .child
            .lock()
            .map_err(|_| "Etat du tour verrouillé".to_string())? = Some(child);
        self.turns
            .lock()
            .map_err(|_| "Etat des conversations verrouillé".to_string())?
            .insert(id, turn.clone());
        self.remember_turn_workspace(id, project_dir.as_deref());
        claim.commit();
        self.runtime_sync.notify(RuntimeSyncTopic::ActiveChatTurns);

        if let Some(mut writer) = stdin.take() {
            let prompt = if account.provider == Provider::OpenCode {
                provider_instructions
                    .as_deref()
                    .map(|instructions| {
                        format!(
                            "Instructions de ce tour :\n{instructions}\n\nDemande utilisateur :\n{prompt}"
                        )
                    })
                    .unwrap_or(prompt)
            } else {
                prompt
            };
            if let Err(error) = writer
                .write_all(prompt.as_bytes())
                .and_then(|_| writer.write_all(b"\n"))
            {
                let _ = self.stop(id);
                return Err(format!("Impossible d'envoyer le message : {error}"));
            }
        }

        let output_turn = turn.clone();
        let provider = account.provider;
        let stdout_thread = thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    apply_provider_event(&output_turn, provider, &line);
                }
            }
            output_turn.output_closed.store(true, Ordering::Release);
            archive_finished_snapshot(&output_turn);
        });

        let error_buffer = Arc::new(Mutex::new(String::new()));
        let error_writer = error_buffer.clone();
        let stderr_thread = thread::spawn(move || {
            if let Some(stderr) = stderr {
                let mut bytes = Vec::new();
                let _ = stderr.take(MAX_ERROR_BYTES as u64).read_to_end(&mut bytes);
                if let Ok(mut target) = error_writer.lock() {
                    *target = String::from_utf8_lossy(&bytes).trim().to_string();
                }
            }
        });

        let supervisor_turn = turn.clone();
        let account_id = account.id.clone();
        let account_label = account.label.clone();
        thread::spawn(move || {
            // Le fichier doit rester lisible pendant tout le tour puis est
            // automatiquement supprimé, succès, erreur ou annulation compris.
            let _skill_file = skill_file;
            let _model_tool_file = model_tool_file;
            let _image_files = image_files;
            let exit = wait_for_child(&supervisor_turn);
            // Un evenement terminal est la derniere sortie utile du provider.
            // Ne pas attendre indefiniment la fermeture de pipes herites par un
            // sous-processus : les handles de thread sont alors simplement
            // detaches. Sans evenement terminal, on conserve le drain complet
            // historique afin de ne perdre aucune derniere ligne JSON.
            if provider_terminal_event(&supervisor_turn).is_none() {
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
            }
            if let Ok(mut child) = supervisor_turn.child.lock() {
                child.take();
            }
            let stderr = error_buffer
                .lock()
                .map(|value| value.clone())
                .unwrap_or_default();
            finish_turn(&supervisor_turn, exit, &stderr);
            let _ = metrics::record_agent_run(
                &account_id,
                &account_label,
                snapshot.started_at,
                metrics::now_ts(),
            );
        });

        Ok(snapshot)
    }

    pub fn status(&self, id: u64) -> Result<ChatTurnSnapshot, String> {
        let turn = self
            .turns
            .lock()
            .map_err(|_| "Etat des conversations verrouillé".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| "Tour de conversation introuvable".to_string())?;
        let snapshot = turn
            .snapshot
            .lock()
            .map_err(|_| "Etat du tour verrouillé".to_string())?;
        let archived = turn
            .archived_snapshot
            .lock()
            .map_err(|_| "Archive du tour verrouillée".to_string())?;
        if let Some(compressed) = archived.as_deref() {
            decode_archived_snapshot(compressed)
        } else {
            Ok(snapshot.clone())
        }
    }

    /// Nombre de tours visibles qui consomment actuellement une place sur le
    /// noeud. Cette valeur alimente le repartiteur multi-VPS.
    pub fn active_count(&self) -> usize {
        self.turns
            .lock()
            .map(|turns| {
                turns
                    .values()
                    .filter(|turn| {
                        turn.snapshot
                            .lock()
                            .map(|snapshot| {
                                matches!(
                                    snapshot.status,
                                    ChatTurnStatus::Running | ChatTurnStatus::Finalizing
                                )
                            })
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    pub fn active(&self) -> Result<Vec<ActiveChatTurnSummary>, String> {
        let workspaces = self
            .workspaces
            .lock()
            .map_err(|_| "Environnements des conversations verrouilles".to_string())?
            .clone();
        let turns = self
            .turns
            .lock()
            .map_err(|_| "Etat des conversations verrouille".to_string())?;
        let mut snapshots = Vec::new();
        for turn in turns.values() {
            let snapshot = turn
                .snapshot
                .lock()
                .map_err(|_| "Etat du tour verrouille".to_string())?;
            if matches!(
                snapshot.status,
                ChatTurnStatus::Running | ChatTurnStatus::Finalizing
            ) {
                snapshots.push(ActiveChatTurnSummary {
                    id: snapshot.id,
                    account_id: snapshot.account_id.clone(),
                    session_id: snapshot.session_id.clone(),
                    source_chat_key: snapshot.source_chat_key.clone(),
                    status: snapshot.status,
                    started_at: snapshot.started_at,
                    waiting_for_user: snapshot.parts.iter().any(part_waits_for_user_input),
                    project_dir: workspaces.get(&snapshot.id).cloned(),
                });
            }
        }
        snapshots.sort_by_key(|snapshot| snapshot.id);
        Ok(snapshots)
    }

    /// Indique si une session fournisseur est encore occupée par un tour.
    /// Les moteurs autonomes l'utilisent pour différer leur reprise sans
    /// interrompre la réponse visible ni lancer deux commandes concurrentes.
    pub(crate) fn session_is_busy(
        &self,
        account_id: &str,
        session_id: &str,
    ) -> Result<bool, String> {
        let turns = self
            .turns
            .lock()
            .map_err(|_| "Etat des conversations verrouillé".to_string())?;
        Ok(turns.values().any(|turn| {
            turn.snapshot
                .lock()
                .map(|snapshot| {
                    matches!(
                        snapshot.status,
                        ChatTurnStatus::Running | ChatTurnStatus::Finalizing
                    ) && snapshot.account_id == account_id
                        && snapshot.session_id.as_deref() == Some(session_id)
                })
                .unwrap_or(false)
        }))
    }

    pub fn stop(&self, id: u64) -> Result<ChatTurnSnapshot, String> {
        let turn = self
            .turns
            .lock()
            .map_err(|_| "Etat des conversations verrouillé".to_string())?
            .get(&id)
            .cloned()
            .ok_or_else(|| "Tour de conversation introuvable".to_string())?;

        let mut active_catalog_changed = false;
        if let Ok(mut snapshot) = turn.snapshot.lock() {
            let before = active_turn_signal_state(&snapshot);
            if snapshot.status == ChatTurnStatus::Running {
                snapshot.status = ChatTurnStatus::Cancelled;
                snapshot.finished_at = Some(metrics::now_ts());
                snapshot.error = None;
                complete_running_activities(&mut snapshot, "cancelled");
                complete_running_thoughts(&mut snapshot, "cancelled");
                complete_running_parts(&mut snapshot, "cancelled");
            }
            active_catalog_changed = before != active_turn_signal_state(&snapshot);
        }
        if active_catalog_changed {
            self.runtime_sync.notify(RuntimeSyncTopic::ActiveChatTurns);
        }
        // Tour pilote par une session Claude persistante : on interrompt le tour
        // courant SANS tuer le process (control_request `interrupt`), pour que la
        // session et ses taches de fond survivent. C'est essentiel sur un VPS
        // partage, ou relancer un process `claude` a chaque annulation gaspille
        // memoire et temps de reprise. Si le tour ne se draine pas a temps, on
        // bascule sur l'arret complet (repli sur.) pour ne jamais laisser une
        // session bloquee.
        if let Some(session) = turn.live_session.upgrade() {
            let drained = session.request_interrupt().is_ok()
                && session.wait_until_parked(LIVE_CLAUDE_INTERRUPT_DRAIN);
            if !drained {
                session.shutdown();
                self.forget_live_session(session.key());
            }
        }
        if let Ok(mut child) = turn.child.lock() {
            if let Some(child) = child.as_mut() {
                terminate_chat_process_tree(child)
                    .map_err(|error| format!("Arret du processus du tour impossible : {error}"))?;
            }
        }
        self.status(id)
    }

    pub fn compact(
        &self,
        account_id: String,
        session_id: String,
    ) -> Result<CompactChatSessionResult, String> {
        let app_settings = settings::load_settings_for_terminal()?;
        let account = app_settings
            .accounts
            .iter()
            .find(|candidate| candidate.id == account_id)
            .cloned()
            .ok_or_else(|| "Compte introuvable".to_string())?;
        if account.provider != Provider::Codex {
            return Err("La commande /compact est disponible uniquement avec Codex".to_string());
        }
        validate_session_id(account.provider, &session_id)?;
        if !settings::account_has_auth_tokens(&account) {
            return Err(format!(
                "Compte non authentifie : {}. Reconnecte ce compte avant de compacter.",
                account.label
            ));
        }

        // Conserve la reservation pendant tout le tour de compaction. Un
        // message normal visant la meme session est ainsi refuse proprement.
        let _claim = self.reserve_session(&account_id, &session_id)?;
        let context_usage =
            run_codex_compaction(&app_settings, &account, &session_id)?.or_else(|| {
                discussions::context_usage_for_account(&account_id, &session_id)
                    .ok()
                    .flatten()
            });
        Ok(CompactChatSessionResult { context_usage })
    }

    fn reserve_turn(&self, request: &StartChatTurnRequest) -> Result<ChatClaim, String> {
        let Some(session_id) = request.session_id.as_deref() else {
            // Sans identifiant fournisseur, la requete cree une conversation
            // neuve et independante. Plusieurs agents peuvent donc demarrer
            // simultanement avec le meme compte sans partager un verrou vide.
            return Ok(ChatClaim {
                claims: self.claims.clone(),
                key: None,
            });
        };
        self.reserve_session(&request.account_id, session_id)
    }

    fn reserve_session(&self, account_id: &str, session_id: &str) -> Result<ChatClaim, String> {
        let key = format!("{account_id}\0{session_id}");
        let mut claims = self
            .claims
            .lock()
            .map_err(|_| "Reservations de conversations verrouillees".to_string())?;
        if claims.contains(&key) {
            return Err("Une réponse est déjà en cours dans cette conversation".to_string());
        }
        if self.session_is_busy(account_id, session_id)? {
            Err("Une réponse est déjà en cours dans cette conversation".to_string())
        } else {
            claims.insert(key.clone());
            Ok(ChatClaim {
                claims: self.claims.clone(),
                key: Some(key),
            })
        }
    }

    fn prune_finished_turns(&self) {
        let Ok(mut turns) = self.turns.lock() else {
            return;
        };
        if turns.len() <= MAX_RETAINED_TURNS {
            return;
        }
        let mut finished = turns
            .iter()
            .filter_map(|(id, turn)| {
                turn.snapshot
                    .lock()
                    .ok()
                    .and_then(|snapshot| snapshot.finished_at.map(|finished| (*id, finished)))
            })
            .collect::<Vec<_>>();
        finished.sort_by_key(|(_, timestamp)| *timestamp);
        let remove = turns.len().saturating_sub(MAX_RETAINED_TURNS);
        for (id, _) in finished.into_iter().take(remove) {
            turns.remove(&id);
            if let Ok(mut owners) = self.owners.lock() {
                owners.remove(&id);
            }
            if let Ok(mut workspaces) = self.workspaces.lock() {
                workspaces.remove(&id);
            }
        }
    }

    /// Route un tour Claude vers le process persistant de la conversation, en
    /// (re)demarrant ce process au besoin. Le tour n'est enregistre (et donc
    /// visible via `status`) qu'apres l'ecriture reussie du message : un echec
    /// anterieur laisse l'appelant retomber sur le one-shot sans effet de bord
    /// (ni reservation consommee, ni tour fantome).
    #[allow(clippy::too_many_arguments)]
    fn start_persistent_claude_turn(
        &self,
        command: Command,
        account: &AccountProfile,
        session_id: &str,
        profile: &str,
        id: u64,
        detail: Option<String>,
        prompt: &str,
        source_chat_key: Option<String>,
    ) -> Result<ChatTurnSnapshot, String> {
        let key = live_claude_key(&account.id, session_id);
        let session = self.acquire_live_claude_session(&key, profile, command)?;

        let snapshot = seed_turn_snapshot(
            id,
            account,
            Some(session_id.to_string()),
            detail,
            source_chat_key,
        );
        let turn = Arc::new(ChatTurn {
            snapshot: Mutex::new(snapshot.clone()),
            archived_snapshot: Mutex::new(None),
            output_closed: AtomicBool::new(false),
            child: Mutex::new(None),
            provider_terminal: Mutex::new(None),
            runtime_sync: self.runtime_sync.clone(),
            live_session: Arc::downgrade(&session),
        });

        // Prise de tour atomique : le lecteur doit connaitre le tour courant
        // avant tout evenement, et un tour precedent en cours de drain (apres une
        // annulation) ne doit pas etre ecrase. Si la session est encore occupee,
        // l'appelant repart proprement en one-shot.
        if !session.try_begin_turn(turn.clone()) {
            return Err(
                "Session Claude persistante occupée (tour précédent en cours de libération)"
                    .to_string(),
            );
        }
        if let Err(error) = session.submit_user_message(prompt) {
            // Le process vient probablement de mourir : on le recycle et on
            // laisse l'appelant repartir en one-shot.
            session.clear_current_turn(&turn);
            session.shutdown();
            self.forget_live_session(&key);
            return Err(error);
        }

        self.turns
            .lock()
            .map_err(|_| "Etat des conversations verrouillé".to_string())?
            .insert(id, turn);
        Ok(snapshot)
    }

    /// Recupere une session vivante compatible (meme profil de lancement) ou en
    /// demarre une neuve. Recycle au passage les sessions mortes ou inactives et
    /// applique le plafond memoire du noeud.
    fn acquire_live_claude_session(
        &self,
        key: &str,
        profile: &str,
        command: Command,
    ) -> Result<Arc<LiveClaudeSession>, String> {
        let mut registry = self
            .live_claude
            .lock()
            .map_err(|_| "Registre des sessions Claude verrouillé".to_string())?;

        registry.retain(|_, session| {
            if session.is_dead() || session.is_idle(live_claude_idle_timeout()) {
                session.shutdown();
                false
            } else {
                true
            }
        });

        if let Some(existing) = registry.get(key) {
            if !existing.is_dead() && existing.profile() == profile {
                existing.touch();
                return Ok(existing.clone());
            }
            // Un flag de lancement a change (mode, modele, instructions, MCP) :
            // le process en cours ne peut pas etre reconfigure a chaud en phase 1,
            // on le relance via `--resume`.
            if let Some(stale) = registry.remove(key) {
                stale.shutdown();
            }
        }

        // Un plafond explicite reste configurable pour les petits VPS. La
        // valeur 0 utilise seulement l'enveloppe memoire globale.
        let max_sessions = live_claude_max_sessions();
        while max_sessions > 0 && registry.len() >= max_sessions {
            let victim = registry
                .iter()
                .min_by_key(|(_, session)| session.last_activity())
                .map(|(victim_key, _)| victim_key.clone());
            match victim {
                Some(victim_key) => {
                    if let Some(session) = registry.remove(&victim_key) {
                        session.shutdown();
                    }
                }
                None => break,
            }
        }

        let session = LiveClaudeSession::launch(key.to_string(), profile.to_string(), command)?;
        registry.insert(key.to_string(), session.clone());
        Ok(session)
    }

    fn forget_live_session(&self, key: &str) {
        if let Ok(mut registry) = self.live_claude.lock() {
            registry.remove(key);
        }
    }
}

impl ChatClaim {
    fn commit(self) {
        // La reservation est maintenant remplacee par le tour Running.
    }
}

impl Drop for ChatClaim {
    fn drop(&mut self) {
        let Some(key) = self.key.as_ref() else {
            return;
        };
        if let Ok(mut claims) = self.claims.lock() {
            claims.remove(key);
        }
    }
}

/// Clef de registre d'une session Claude persistante : couple
/// `(compte, session)`. Le `\0` ne peut apparaitre ni dans un id de compte ni
/// dans un id de session.
fn live_claude_key(account_id: &str, session_id: &str) -> String {
    format!("{account_id}\u{0}{session_id}")
}

/// Une ligne stream-json de message utilisateur, envoyee sur le stdin du process
/// persistant. Le contenu est un texte simple ; l'echappement JSON garantit une
/// unique ligne physique meme si le prompt contient des sauts de ligne.
fn live_claude_user_message_line(prompt: &str) -> String {
    let payload = json!({
        "type": "user",
        "message": { "role": "user", "content": prompt },
    });
    format!("{payload}\n")
}

/// Empreinte des parametres de lancement figes du process Claude (non modifia-
/// bles a chaud en phase 1). Deux tours au meme profil partagent le meme
/// process ; un profil different force un relance via `--resume`.
fn live_claude_profile(
    canonical_home: &Path,
    execution_dir: Option<&Path>,
    mode: ChatTurnMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    provider_instructions: Option<&str>,
) -> String {
    format!(
        "{}\u{0}{}\u{0}{:?}\u{0}{}\u{0}{}\u{0}{}",
        canonical_home.display(),
        execution_dir
            .map(|dir| dir.display().to_string())
            .unwrap_or_default(),
        mode,
        model.unwrap_or(""),
        reasoning_effort.unwrap_or(""),
        provider_instructions.unwrap_or(""),
    )
}

/// Snapshot initial commun aux tours one-shot et persistants : etat `Running`
/// avec l'activite et la pensee d'amorce affichees pendant la preparation.
fn seed_turn_snapshot(
    id: u64,
    account: &AccountProfile,
    session_id: Option<String>,
    detail: Option<String>,
    source_chat_key: Option<String>,
) -> ChatTurnSnapshot {
    ChatTurnSnapshot {
        id,
        account_id: account.id.clone(),
        session_id,
        source_chat_key,
        status: ChatTurnStatus::Running,
        started_at: metrics::now_ts(),
        finished_at: None,
        error: None,
        activities: vec![ChatActivity {
            id: "agent-start".to_string(),
            kind: "think".to_string(),
            label: format!("{} prépare la réponse", provider_label(account.provider)),
            detail,
            status: "running".to_string(),
        }],
        thoughts: vec![ChatThought {
            id: "agent-thinking".to_string(),
            kind: "reasoning".to_string(),
            text: format!(
                "{} analyse la demande et prépare la prochaine étape.",
                provider_label(account.provider)
            ),
            status: "running".to_string(),
        }],
        parts: Vec::new(),
    }
}

/// Un process `claude --print --input-format stream-json` maintenu vivant pour
/// toute une conversation. Les tours y sont ecrits en serie (un seul actif a la
/// fois, garanti par `reserve_session`) ; un lecteur permanent route chaque
/// evenement vers le tour courant et finalise ce dernier sur l'evenement
/// `result` sans arreter le process, de sorte que les taches shell en
/// arriere-plan survivent d'un tour a l'autre.
struct LiveClaudeSession {
    key: String,
    profile: String,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    current_turn: Mutex<Option<Arc<ChatTurn>>>,
    stderr_tail: Arc<Mutex<String>>,
    dead: Arc<AtomicBool>,
    last_activity: AtomicI64,
}

impl LiveClaudeSession {
    fn launch(key: String, profile: String, mut command: Command) -> Result<Arc<Self>, String> {
        let mut child = command.spawn().map_err(|error| {
            format!("Impossible de lancer la session Claude persistante : {error}")
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Session Claude sans sortie standard".to_string())?;
        let stderr = child.stderr.take();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Session Claude sans entrée standard".to_string())?;

        let session = Arc::new(Self {
            key,
            profile,
            child: Mutex::new(Some(child)),
            stdin: Mutex::new(Some(stdin)),
            current_turn: Mutex::new(None),
            stderr_tail: Arc::new(Mutex::new(String::new())),
            dead: Arc::new(AtomicBool::new(false)),
            last_activity: AtomicI64::new(metrics::now_ts()),
        });

        spawn_live_claude_reader(session.clone(), stdout);
        if let Some(stderr) = stderr {
            spawn_live_claude_stderr(session.stderr_tail.clone(), stderr);
        }
        Ok(session)
    }

    fn key(&self) -> &str {
        &self.key
    }

    fn profile(&self) -> &str {
        &self.profile
    }

    fn touch(&self) {
        self.last_activity
            .store(metrics::now_ts(), Ordering::Release);
    }

    fn last_activity(&self) -> i64 {
        self.last_activity.load(Ordering::Acquire)
    }

    /// Vrai si le process est arrete (drapeau pose par le lecteur/`shutdown`) ou
    /// s'il s'est termine sans que l'EOF ait encore ete observe.
    fn is_dead(&self) -> bool {
        if self.dead.load(Ordering::Acquire) {
            return true;
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(child) = guard.as_mut() {
                if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
                    self.dead.store(true, Ordering::Release);
                    return true;
                }
                return false;
            }
        }
        true
    }

    /// Inactive depuis plus de `timeout` ET sans tour en cours : recyclable.
    fn is_idle(&self, timeout: Duration) -> bool {
        let idle_secs = metrics::now_ts().saturating_sub(self.last_activity());
        idle_secs >= timeout.as_secs() as i64 && self.current_turn_is_empty()
    }

    fn current_turn_is_empty(&self) -> bool {
        self.current_turn
            .lock()
            .map(|guard| guard.is_none())
            .unwrap_or(true)
    }

    /// Prend le tour comme tour courant seulement si aucun autre ne l'occupe
    /// (test-et-pose atomique sous le meme verrou). Empeche d'ecraser un tour
    /// precedent encore en cours de drain apres une interruption.
    fn try_begin_turn(&self, turn: Arc<ChatTurn>) -> bool {
        if let Ok(mut guard) = self.current_turn.lock() {
            if guard.is_some() {
                return false;
            }
            *guard = Some(turn);
            self.touch();
            return true;
        }
        false
    }

    /// Retire le tour courant seulement s'il s'agit toujours du meme (evite
    /// d'ecraser un tour suivant deja soumis dans une course).
    fn clear_current_turn(&self, turn: &Arc<ChatTurn>) {
        if let Ok(mut guard) = self.current_turn.lock() {
            if guard
                .as_ref()
                .is_some_and(|current| Arc::ptr_eq(current, turn))
            {
                guard.take();
            }
        }
    }

    fn submit_user_message(&self, prompt: &str) -> Result<(), String> {
        let line = live_claude_user_message_line(prompt);
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| "Entrée de la session Claude verrouillée".to_string())?;
        let writer = guard
            .as_mut()
            .ok_or_else(|| "Session Claude déjà fermée".to_string())?;
        if let Err(error) = writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
        {
            self.dead.store(true, Ordering::Release);
            return Err(format!("Écriture du message impossible : {error}"));
        }
        self.touch();
        Ok(())
    }

    /// Demande au process d'interrompre le tour en cours sans le tuer. En mode
    /// stream-json, le CLI lit stdin en continu (meme pendant un tour) et traite
    /// ce control_request : il avorte le tour puis emet un evenement `result`,
    /// que le lecteur utilise comme frontiere pour liberer la session.
    fn request_interrupt(&self) -> Result<(), String> {
        let payload = json!({
            "type": "control_request",
            "request_id": Uuid::new_v4().to_string(),
            "request": { "subtype": "interrupt" },
        });
        let line = format!("{payload}\n");
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| "Entrée de la session Claude verrouillée".to_string())?;
        let writer = guard
            .as_mut()
            .ok_or_else(|| "Session Claude déjà fermée".to_string())?;
        writer
            .write_all(line.as_bytes())
            .and_then(|_| writer.flush())
            .map_err(|error| {
                self.dead.store(true, Ordering::Release);
                format!("Interruption impossible : {error}")
            })
    }

    /// Attend (borne) que le lecteur ait finalise et libere le tour courant apres
    /// une interruption. Vrai si la session est de nouveau libre et vivante.
    fn wait_until_parked(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if self.is_dead() {
                return false;
            }
            if self.current_turn_is_empty() {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn stderr_snapshot(&self) -> String {
        self.stderr_tail
            .lock()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    /// Arrete definitivement le process (et ses taches de fond) puis le reape.
    fn shutdown(&self) {
        self.dead.store(true, Ordering::Release);
        if let Ok(mut guard) = self.stdin.lock() {
            guard.take();
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = terminate_chat_process_tree(&mut child);
                let _ = child.wait();
            }
        }
    }
}

/// Vrai si la ligne stream-json est l'evenement `result` de Claude, qui clot un
/// tour (succes, erreur ou interruption). C'est la frontiere fiable entre deux
/// tours d'un meme process persistant.
fn claude_line_is_turn_boundary(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(Value::as_str)
                .map(|event_type| event_type == "result")
        })
        .unwrap_or(false)
}

/// Lecteur permanent d'une session Claude persistante. Route chaque evenement
/// stream-json vers le tour courant, finalise ce tour sur l'evenement terminal
/// (`result`) sans arreter le process, puis attend le tour suivant. A la
/// fermeture du flux (process termine), marque la session morte et fait echouer
/// un eventuel tour encore en cours.
fn spawn_live_claude_reader(session: Arc<LiveClaudeSession>, stdout: ChildStdout) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Some(turn) = session
                .current_turn
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
            else {
                // Aucun tour actif : evenement inter-tours, ignore.
                continue;
            };
            apply_provider_event(&turn, Provider::Claude, &line);
            // Frontiere de tour = evenement `result` de Claude. On le detecte sur
            // la ligne elle-meme plutot que via le statut : un tour deja passe en
            // `Cancelled` (interruption) n'ecrit pas d'evenement terminal, mais
            // doit tout de meme liberer la session a l'arrivee de son `result`.
            if claude_line_is_turn_boundary(&line) {
                // Finalise le tour mais garde le process (les taches de fond
                // continuent d'exister). `finish_turn` respecte un statut deja
                // terminal (tour annule) et le laisse tel quel.
                turn.output_closed.store(true, Ordering::Release);
                finish_turn(&turn, Err(String::new()), &session.stderr_snapshot());
                session.clear_current_turn(&turn);
                session.touch();
            }
        }

        // Flux ferme => le process `claude` s'est arrete.
        session.dead.store(true, Ordering::Release);
        if let Ok(mut guard) = session.current_turn.lock() {
            if let Some(turn) = guard.take() {
                let still_running = turn
                    .snapshot
                    .lock()
                    .map(|snapshot| {
                        matches!(
                            snapshot.status,
                            ChatTurnStatus::Running | ChatTurnStatus::Finalizing
                        )
                    })
                    .unwrap_or(false);
                if still_running {
                    turn.output_closed.store(true, Ordering::Release);
                    finish_turn(
                        &turn,
                        Err("Le processus Claude persistant s'est arrêté".to_string()),
                        &session.stderr_snapshot(),
                    );
                }
            }
        }
    });
}

/// Accumule la sortie d'erreur du process persistant dans un tampon borne, pour
/// l'associer a un tour qui echouerait.
fn spawn_live_claude_stderr(stderr_tail: Arc<Mutex<String>>, stderr: std::process::ChildStderr) {
    thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if let Ok(mut tail) = stderr_tail.lock() {
                if !tail.is_empty() {
                    tail.push('\n');
                }
                tail.push_str(&line);
                if tail.len() > MAX_ERROR_BYTES {
                    // Ne garde que la fin, en s'alignant sur une frontiere de
                    // caractere pour ne jamais couper au milieu d'un UTF-8.
                    let mut cut = tail.len() - MAX_ERROR_BYTES;
                    while cut < tail.len() && !tail.is_char_boundary(cut) {
                        cut += 1;
                    }
                    *tail = tail.split_off(cut);
                }
            }
        }
    });
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn start_chat_turn(
    state: State<'_, ChatTurnManager>,
    account_id: String,
    session_id: Option<String>,
    prompt: String,
    image_attachments: Option<Vec<ChatImageAttachmentRequest>>,
    project_dir: Option<String>,
    mode: Option<ChatTurnMode>,
    model: Option<String>,
    reasoning_effort: Option<String>,
    agent_tools: Option<Vec<ChatAgentTool>>,
    agent_skills: Option<Vec<ChatAgentSkill>>,
    question_tool: Option<bool>,
    proof_tool: Option<bool>,
    source_chat_key: Option<String>,
) -> Result<ChatTurnSnapshot, String> {
    state.start(StartChatTurnRequest {
        account_id,
        session_id,
        prompt,
        image_attachments: image_attachments.unwrap_or_default(),
        project_dir,
        mode: mode.unwrap_or_default(),
        model,
        reasoning_effort,
        app_connectors: None,
        app_write_approved: false,
        agent_tools: agent_tools.unwrap_or_default(),
        agent_skills: agent_skills.unwrap_or_default(),
        question_tool: question_tool.unwrap_or(false),
        proof_tool: proof_tool.unwrap_or(false),
        source_chat_key,
    })
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn chat_turn_status(
    state: State<'_, ChatTurnManager>,
    id: u64,
) -> Result<ChatTurnSnapshot, String> {
    state.status(id)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn list_active_chat_turns(
    state: State<'_, ChatTurnManager>,
) -> Result<Vec<ActiveChatTurnSummary>, String> {
    state.active()
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn stop_chat_turn(
    state: State<'_, ChatTurnManager>,
    id: u64,
) -> Result<ChatTurnSnapshot, String> {
    state.stop(id)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub async fn compact_chat_session(
    state: State<'_, ChatTurnManager>,
    account_id: String,
    session_id: String,
) -> Result<CompactChatSessionResult, String> {
    let manager = state.inner().clone();
    tokio::task::spawn_blocking(move || manager.compact(account_id, session_id))
        .await
        .map_err(|error| error.to_string())?
}

fn run_codex_compaction(
    app_settings: &AppSettings,
    account: &AccountProfile,
    session_id: &str,
) -> Result<Option<DiscussionContextUsage>, String> {
    let canonical_home = settings::expand_home(&account.codex_home)?;
    let mut command = settings::codex_app_server_command(app_settings);
    configure_environment(&mut command, app_settings, account, &canonical_home, None);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hide_process_window(&mut command);

    let mut child = command
        .spawn()
        .map_err(|error| format!("Impossible de lancer codex app-server : {error}"))?;
    let Some(mut stdin) = child.stdin.take() else {
        let _ = terminate_chat_process_tree(&mut child);
        return Err("Entree de codex app-server indisponible".to_string());
    };
    let Some(stdout) = child.stdout.take() else {
        let _ = terminate_chat_process_tree(&mut child);
        return Err("Sortie de codex app-server indisponible".to_string());
    };
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let result = (|| -> Result<(Option<DiscussionContextUsage>, String), String> {
        let initialize_request = json!({
            "method": "initialize",
            "id": 1,
            "params": {
                "clientInfo": {
                    "name": "codex_switch_terminal",
                    "title": "Codex Switch Terminal",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": { "experimentalApi": true }
            }
        });
        writeln!(stdin, "{initialize_request}")
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("Initialisation de /compact impossible : {error}"))?;

        let deadline = Instant::now() + COMPACT_TIMEOUT;
        let initialize_response = loop {
            let value = receive_app_server_value(&rx, deadline)?;
            if value.get("id").and_then(Value::as_i64) == Some(1) {
                break value;
            }
        };
        if initialize_response.get("error").is_some() {
            return Err(app_server_error_message(
                &initialize_response,
                "Initialisation de codex app-server impossible",
            ));
        }

        for request in [
            json!({ "method": "initialized", "params": {} }),
            json!({
                "method": "thread/resume",
                "id": 2,
                "params": {
                    "threadId": session_id,
                    "excludeTurns": true
                }
            }),
        ] {
            writeln!(stdin, "{request}")
                .map_err(|error| format!("Envoi de /compact impossible : {error}"))?;
        }
        stdin
            .flush()
            .map_err(|error| format!("Envoi de /compact impossible : {error}"))?;

        let resume_response = loop {
            let value = receive_app_server_value(&rx, deadline)?;
            if value.get("id").and_then(Value::as_i64) == Some(2) {
                break value;
            }
        };
        if resume_response.get("error").is_some() {
            return Err(app_server_error_message(
                &resume_response,
                "Impossible de reprendre cette conversation",
            ));
        }
        let resumed_thread_id = resume_response
            .pointer("/result/thread/id")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(session_id)
            .to_string();

        let compact_request = json!({
            "method": "thread/compact/start",
            "id": 3,
            "params": { "threadId": resumed_thread_id }
        });
        writeln!(stdin, "{compact_request}")
            .and_then(|_| stdin.flush())
            .map_err(|error| format!("Envoi de /compact impossible : {error}"))?;

        let mut accepted = false;
        let mut completed = false;
        let mut latest_usage = None;
        while !accepted || !completed {
            let value = receive_app_server_value(&rx, deadline)?;
            if value.get("id").and_then(Value::as_i64) == Some(3) {
                if value.get("error").is_some() {
                    return Err(app_server_error_message(
                        &value,
                        "La compaction Codex a ete refusee",
                    ));
                }
                accepted = true;
                continue;
            }
            if value.get("method").and_then(Value::as_str) == Some("thread/tokenUsage/updated")
                && value.pointer("/params/threadId").and_then(Value::as_str)
                    == Some(resumed_thread_id.as_str())
            {
                if let Some(usage) = context_usage_from_app_server_notification(&value) {
                    latest_usage = Some(usage);
                }
                continue;
            }
            if value.get("method").and_then(Value::as_str) != Some("turn/completed")
                || value.pointer("/params/threadId").and_then(Value::as_str)
                    != Some(resumed_thread_id.as_str())
            {
                continue;
            }
            match value
                .pointer("/params/turn/status")
                .and_then(Value::as_str)
                .unwrap_or("failed")
            {
                "completed" => completed = true,
                "interrupted" => return Err("La compaction a ete interrompue".to_string()),
                _ => {
                    return Err(value
                        .pointer("/params/turn/error/message")
                        .and_then(Value::as_str)
                        .unwrap_or("La compaction Codex a echoue")
                        .to_string())
                }
            }
        }
        Ok((latest_usage, resumed_thread_id))
    })();

    drop(stdin);
    let _ = terminate_chat_process_tree(&mut child);
    let _ = child.wait();

    result.map(|(usage, resumed_thread_id)| {
        usage.or_else(|| {
            discussions::context_usage_for_account(&account.id, &resumed_thread_id)
                .ok()
                .flatten()
        })
    })
}

fn receive_app_server_value(
    rx: &mpsc::Receiver<String>,
    deadline: Instant,
) -> Result<Value, String> {
    loop {
        let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
            return Err("La compaction Codex a depasse 3 minutes".to_string());
        };
        match rx.recv_timeout(remaining) {
            Ok(line) => {
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    return Ok(value);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err("La compaction Codex a depasse 3 minutes".to_string())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("codex app-server s'est arrete pendant la compaction".to_string())
            }
        }
    }
}

fn app_server_error_message(value: &Value, fallback: &str) -> String {
    value
        .pointer("/error/message")
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty())
        .unwrap_or(fallback)
        .chars()
        .take(1200)
        .collect()
}

fn context_usage_from_app_server_notification(value: &Value) -> Option<DiscussionContextUsage> {
    let used_tokens = value
        .pointer("/params/tokenUsage/last/totalTokens")
        .and_then(nonnegative_json_u64)?;
    let context_window = value
        .pointer("/params/tokenUsage/modelContextWindow")
        .and_then(nonnegative_json_u64)?;
    DiscussionContextUsage::from_counts(used_tokens, context_window)
}

fn nonnegative_json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| value.as_f64().map(|number| number.max(0.0) as u64))
}

#[cfg(test)]
fn configure_provider_command(
    command: &mut Command,
    account: &AccountProfile,
    session_id: Option<&str>,
    mode: ChatTurnMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    app_connectors: Option<&[ChatAppConnector]>,
    app_write_approved: bool,
    environment_instructions: Option<&str>,
    model_tool_server: Option<&ChatModelToolServerConfig>,
    model_tool_config_path: Option<&Path>,
) {
    configure_provider_command_with_images(
        command,
        account,
        session_id,
        mode,
        model,
        reasoning_effort,
        app_connectors,
        app_write_approved,
        environment_instructions,
        model_tool_server,
        model_tool_config_path,
        &[],
    );
}

fn configure_provider_command_with_images(
    command: &mut Command,
    account: &AccountProfile,
    session_id: Option<&str>,
    mode: ChatTurnMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    app_connectors: Option<&[ChatAppConnector]>,
    app_write_approved: bool,
    environment_instructions: Option<&str>,
    model_tool_server: Option<&ChatModelToolServerConfig>,
    model_tool_config_path: Option<&Path>,
    image_paths: &[PathBuf],
) {
    configure_provider_command_with_images_and_scope(
        command,
        account,
        session_id,
        mode,
        model,
        reasoning_effort,
        app_connectors,
        app_write_approved,
        environment_instructions,
        model_tool_server,
        model_tool_config_path,
        image_paths,
        ChatFilesystemScope::Default,
    );
}

fn configure_provider_command_with_images_and_scope(
    command: &mut Command,
    account: &AccountProfile,
    session_id: Option<&str>,
    mode: ChatTurnMode,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
    app_connectors: Option<&[ChatAppConnector]>,
    app_write_approved: bool,
    environment_instructions: Option<&str>,
    model_tool_server: Option<&ChatModelToolServerConfig>,
    model_tool_config_path: Option<&Path>,
    image_paths: &[PathBuf],
    filesystem_scope: ChatFilesystemScope,
) {
    match account.provider {
        // Inatteignable : `ChatTurnManager::start` refuse freebuff en amont.
        Provider::Freebuff | Provider::Aihubmix => {}
        Provider::Codex => {
            command.arg("exec");
            if session_id.is_some() {
                command.arg("resume");
            }
            // La memoire automatique locale du CLI est desactivee par defaut.
            // Chaque compte conserve son store dans son CODEX_HOME ; le cwd du
            // tour permet ensuite a Codex de retrouver le contexte pertinent.
            command.arg("--enable").arg("memories");
            if filesystem_scope != ChatFilesystemScope::OrchestrationWorkspace {
                command
                    .arg("--enable")
                    .arg("multi_agent")
                    .arg("--enable")
                    .arg("multi_agent_v2");
            } else {
                command
                    .arg("--disable")
                    .arg("multi_agent")
                    .arg("--disable")
                    .arg("multi_agent_v2");
            }
            command.arg("--json");
            // Expose uniquement les resumes prevus pour l'utilisateur. Le
            // raisonnement interne brut reste volontairement masque.
            command
                .arg("-c")
                .arg("hide_agent_reasoning=false")
                .arg("-c")
                .arg("show_raw_agent_reasoning=false");
            configure_codex_app_connectors(command, app_connectors, app_write_approved);
            configure_codex_model_tool(command, model_tool_server);
            // Le bypass global court-circuiterait aussi les demandes
            // d'approbation des apps. Un agent avec connecteurs conserve donc
            // le sandbox normal, meme si le compte autorise le bypass ailleurs.
            if filesystem_scope == ChatFilesystemScope::ReviewProofArtifacts {
                // Le cwd est `.codex-proof/` pour ce tour. `workspace-write`
                // autorise donc la capture sans rendre le projet parent
                // modifiable, meme si le compte utilise normalement le bypass.
                command
                    .arg("-C")
                    .arg(".")
                    .arg("-c")
                    .arg("sandbox_mode=\"workspace-write\"")
                    .arg("-c")
                    .arg("approval_policy=\"never\"")
                    .arg("-c")
                    .arg("sandbox_workspace_write.network_access=false");
            } else if filesystem_scope == ChatFilesystemScope::OrchestrationWorkspace {
                command.arg("-C").arg(".");
                if matches!(mode, ChatTurnMode::Plan | ChatTurnMode::Ask) {
                    command
                        .arg("-c")
                        .arg("sandbox_mode=\"read-only\"")
                        .arg("-c")
                        .arg("approval_policy=\"never\"");
                } else {
                    command
                        .arg("-c")
                        .arg("sandbox_mode=\"workspace-write\"")
                        .arg("-c")
                        .arg("approval_policy=\"never\"")
                        .arg("-c")
                        .arg("sandbox_workspace_write.network_access=false");
                }
            } else if matches!(mode, ChatTurnMode::Plan | ChatTurnMode::Ask) {
                command.arg("-c").arg("sandbox_mode=\"read-only\"");
            } else if account.bypass
                && app_connectors.is_none_or(|connectors| connectors.is_empty())
            {
                command.arg(account.provider.bypass_flag());
            }
            if let Some(model) = model {
                command.arg("--model").arg(model);
            }
            if let Some(effort) = reasoning_effort {
                command
                    .arg("-c")
                    .arg(format!("model_reasoning_effort=\"{effort}\""));
            }
            if let Some(instructions) = environment_instructions {
                command.arg("-c").arg(format!(
                    "developer_instructions={}",
                    serde_json::to_string(instructions)
                        .expect("une chaine Rust est toujours serialisable en JSON")
                ));
            }
            for path in image_paths {
                command.arg("--image").arg(path);
            }
            if let Some(session_id) = session_id {
                command.arg(session_id);
            }
            command.arg("-");
        }
        Provider::Claude => {
            command
                .arg("--print")
                .arg("--output-format")
                .arg("stream-json")
                .arg("--verbose");
            if let Some(session_id) = session_id {
                command.arg("--resume").arg(session_id);
            }
            match mode {
                ChatTurnMode::Plan | ChatTurnMode::Ask => {
                    command.arg("--permission-mode").arg("plan");
                }
                ChatTurnMode::Build
                    if account.bypass
                        && filesystem_scope != ChatFilesystemScope::OrchestrationWorkspace =>
                {
                    command.arg(account.provider.bypass_flag());
                }
                ChatTurnMode::Build => {
                    command.arg("--permission-mode").arg("acceptEdits");
                }
            }
            if let Some(model) = model {
                command.arg("--model").arg(model);
            }
            // Intensite de raisonnement Claude Code (low/medium/high/xhigh/max).
            // Le CLI ignore une valeur inconnue avec un simple avertissement, mais
            // l'UI ne propose que des niveaux valides.
            if let Some(effort) = reasoning_effort {
                command.arg("--effort").arg(effort);
            }
            if let Some(instructions) = environment_instructions {
                command.arg("--append-system-prompt").arg(instructions);
            }
            if let Some(path) = model_tool_config_path {
                command
                    .arg("--mcp-config")
                    .arg(path)
                    .arg("--allowedTools")
                    .arg(format!(
                        "mcp__{MCP_SERVER_NAME}__{CREATE_GOAL_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{AUTONOMOUS_AGENT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{UPDATE_AUTONOMOUS_AGENT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{PAUSE_AUTONOMOUS_AGENT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{CREATE_CHAT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{LIST_OUTLOOK_MESSAGES_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{LIST_CALENDAR_EVENTS_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{SEND_OUTLOOK_EMAIL_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{CREATE_CALENDAR_EVENT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{UPDATE_CALENDAR_EVENT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME},mcp__{MCP_SERVER_NAME}__{QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME}"
                    ));
            }
        }
        Provider::OpenCode => {
            configure_opencode_model_tool(command, model_tool_server);
            command
                .arg("run")
                .arg("--format")
                .arg("json")
                .arg("--thinking");
            if let Some(session_id) = session_id {
                command.arg("--session").arg(session_id);
            }
            command.arg("--agent").arg(match mode {
                ChatTurnMode::Plan | ChatTurnMode::Ask => "plan",
                ChatTurnMode::Build => "build",
            });
            if matches!(mode, ChatTurnMode::Build)
                && account.bypass
                && filesystem_scope != ChatFilesystemScope::OrchestrationWorkspace
            {
                command.arg(account.provider.bypass_flag());
            }
            if let Some(model) = model {
                command.arg("--model").arg(model);
            }
            if account
                .inference_provider
                .as_deref()
                .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter"))
            {
                if let Some(effort) = reasoning_effort {
                    command.arg("--variant").arg(effort);
                }
            }
        }
        // HTTP : aucun binaire CLI a configurer (voir le runner dedie).
        Provider::OpenAiCompatible => {}
    }
}

fn configure_codex_model_tool(command: &mut Command, config: Option<&ChatModelToolServerConfig>) {
    let Some(config) = config else {
        return;
    };
    command.env(MCP_BEARER_ENV, &config.bearer_token);
    let prefix = format!("mcp_servers.{MCP_SERVER_NAME}");
    let url =
        serde_json::to_string(&config.url).expect("une URL Rust est toujours serialisable en JSON");
    // send_chat_message n'est ajoute aux outils exposes que si le canal a ete
    // explicitement active (CST_AGENT_CHAT_POST=1), meme chose cote liste MCP.
    let mut enabled_tool_names = vec![
        AUTONOMOUS_AGENT_TOOL_NAME,
        UPDATE_AUTONOMOUS_AGENT_TOOL_NAME,
        PAUSE_AUTONOMOUS_AGENT_TOOL_NAME,
        ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME,
        APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME,
        CREATE_CHAT_TOOL_NAME,
        LIST_OUTLOOK_MESSAGES_TOOL_NAME,
        LIST_CALENDAR_EVENTS_TOOL_NAME,
        SEND_OUTLOOK_EMAIL_TOOL_NAME,
        CREATE_CALENDAR_EVENT_TOOL_NAME,
        UPDATE_CALENDAR_EVENT_TOOL_NAME,
        LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME,
        LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME,
        MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME,
        SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME,
        PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
        SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME,
        LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME,
        QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME,
    ];
    if crate::chat_model_tools::chat_post_tool_enabled() {
        enabled_tool_names.push(SEND_CHAT_MESSAGE_TOOL_NAME);
    }
    let enabled_tools_list = enabled_tool_names
        .iter()
        .map(|name| format!("\"{name}\""))
        .collect::<Vec<_>>()
        .join(",");
    let mut model_tool_values = vec![
        format!("{prefix}.url={url}"),
        format!("{prefix}.bearer_token_env_var=\"{MCP_BEARER_ENV}\""),
        format!("{prefix}.enabled_tools=[{enabled_tools_list}]"),
        format!("{prefix}.enabled=true"),
        format!("{prefix}.required=false"),
        format!("{prefix}.startup_timeout_sec={CHAT_MCP_STARTUP_TIMEOUT_SECONDS}"),
        format!("{prefix}.tool_timeout_sec=30"),
        format!("{prefix}.default_tools_approval_mode=\"approve\""),
        format!("{prefix}.tools.{AUTONOMOUS_AGENT_TOOL_NAME}.approval_mode=\"approve\""),
        format!(
            "{prefix}.tools.{UPDATE_AUTONOMOUS_AGENT_TOOL_NAME}.approval_mode=\"approve\""
        ),
        format!(
            "{prefix}.tools.{PAUSE_AUTONOMOUS_AGENT_TOOL_NAME}.approval_mode=\"approve\""
        ),
        format!(
            "{prefix}.tools.{ACTIVATE_SUPERVISOR_GENERAL_REPORT_TOOL_NAME}.approval_mode=\"approve\""
        ),
        format!(
            "{prefix}.tools.{APPLY_AUTONOMOUS_AGENT_POLICY_TOOL_NAME}.approval_mode=\"approve\""
        ),
        format!("{prefix}.tools.{CREATE_CHAT_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{LIST_OUTLOOK_MESSAGES_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{LIST_CALENDAR_EVENTS_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{SEND_OUTLOOK_EMAIL_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{CREATE_CALENDAR_EVENT_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{UPDATE_CALENDAR_EVENT_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{LIST_TIKTOK_DM_CAMPAIGNS_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{LIST_TIKTOK_SENDER_ACCOUNTS_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{MANAGE_TIKTOK_SENDER_LOGIN_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{SELECT_TIKTOK_SENDER_ACCOUNT_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{PREPARE_TIKTOK_DM_CAMPAIGN_TOOL_NAME}.approval_mode=\"approve\""),
        format!("{prefix}.tools.{SEND_TIKTOK_DM_CAMPAIGN_TOOL_NAME}.approval_mode=\"approve\""),
        format!(
            "{prefix}.tools.{LIST_TIKTOK_FOLLOWER_EXTRACTIONS_TOOL_NAME}.approval_mode=\"approve\""
        ),
        format!(
            "{prefix}.tools.{QUEUE_TIKTOK_FOLLOWER_EXTRACTION_TOOL_NAME}.approval_mode=\"approve\""
        ),
    ];
    if crate::chat_model_tools::chat_post_tool_enabled() {
        model_tool_values.push(format!(
            "{prefix}.tools.{SEND_CHAT_MESSAGE_TOOL_NAME}.approval_mode=\"approve\""
        ));
    }
    for value in model_tool_values {
        command.arg("-c").arg(value);
    }
}

fn configure_opencode_model_tool(
    command: &mut Command,
    config: Option<&ChatModelToolServerConfig>,
) {
    let Some(config) = config else {
        return;
    };
    // OpenCode fusionne OPENCODE_CONFIG_CONTENT au-dessus de la configuration
    // du compte. Le jeton ephemere reste uniquement dans l'environnement du
    // processus du tour : aucun secret n'est ecrit dans le dossier utilisateur.
    command.env(MCP_BEARER_ENV, &config.bearer_token);
    let overlay = json!({
        "mcp": {
            (MCP_SERVER_NAME): {
                "type": "remote",
                "url": config.url.clone(),
                "enabled": true,
                "oauth": false,
                "timeout": 30_000,
                "headers": {
                    "Authorization": format!("Bearer {{env:{MCP_BEARER_ENV}}}")
                }
            }
        }
    });
    merge_opencode_config_content(command, overlay);
}

fn deep_merge_json(target: &mut Value, overlay: Value) {
    match (target, overlay) {
        (Value::Object(target), Value::Object(overlay)) => {
            for (key, value) in overlay {
                match target.get_mut(&key) {
                    Some(existing) => deep_merge_json(existing, value),
                    None => {
                        target.insert(key, value);
                    }
                }
            }
        }
        (target, overlay) => *target = overlay,
    }
}

fn opencode_config_content_with_inherited<F>(command: &Command, inherited: F) -> Value
where
    F: FnOnce() -> Option<String>,
{
    let explicit = command.get_envs().find_map(|(key, value)| {
        (key == OPENCODE_CONFIG_CONTENT_ENV).then(|| {
            value.and_then(|value| value.to_str()).map(str::to_owned)
        })
    });
    let content = match explicit {
        // Une suppression explicite ne doit jamais retomber sur l'environnement
        // parent. Elle fournit simplement une base vide aux overlays du tour.
        Some(content) => content,
        None => inherited(),
    };
    // Historiquement les overlays Switch remplacaient completement cette
    // variable. Un JSON herite invalide ne doit donc pas rendre un tour qui
    // fonctionnait auparavant impossible : il est remplace par l'overlay sain.
    content
        .and_then(|content| serde_json::from_str(&content).ok())
        .unwrap_or_else(|| json!({}))
}

fn merge_opencode_config_content(command: &mut Command, overlay: Value) {
    merge_opencode_config_content_with_inherited(command, overlay, || {
        std::env::var(OPENCODE_CONFIG_CONTENT_ENV).ok()
    });
}

fn merge_opencode_config_content_with_inherited<F>(
    command: &mut Command,
    overlay: Value,
    inherited: F,
) where
    F: FnOnce() -> Option<String>,
{
    let mut document = opencode_config_content_with_inherited(command, inherited);
    deep_merge_json(&mut document, overlay);
    command.env(OPENCODE_CONFIG_CONTENT_ENV, document.to_string());
}

/// Ajoute le modele OpenRouter selectionne a l'overlay propre au processus.
/// L'overlay MCP goal eventuellement pose juste avant est lu puis enrichi : il
/// n'est jamais ecrase. Seuls l'identifiant public du modele et l'effort choisi
/// sont ajoutes ; la cle API et le proxy restent dans leur canal serveur dedie.
fn configure_opencode_model_overlay(
    command: &mut Command,
    account: &AccountProfile,
    model: Option<&str>,
    reasoning_effort: Option<&str>,
) -> Result<(), String> {
    if account.provider != Provider::OpenCode {
        return Ok(());
    }
    let inference_provider = account.inference_provider.as_deref().unwrap_or_default();

    let Some(model) = model else {
        return Ok(());
    };
    let effort = reasoning_effort
        .map(str::trim)
        .filter(|effort| !effort.is_empty());
    if let Some(effort) = effort {
        if !settings::is_valid_reasoning_effort(effort) {
            return Err("Intensite de raisonnement invalide".to_string());
        }
    }

    // OpenRouter : le niveau est porte par une variante de modele.
    if inference_provider.eq_ignore_ascii_case("openrouter") {
        let slug = model
            .strip_prefix("openrouter/")
            .filter(|slug| settings::safe_openrouter_model_slug(slug))
            .ok_or_else(|| "Nom de modele OpenRouter invalide".to_string())?;

        let model_config = match effort {
            Some(effort) => json!({
                "variants": {
                    (effort): { "reasoning": { "effort": effort } }
                }
            }),
            None => json!({}),
        };
        merge_opencode_config_content(
            command,
            json!({
                "provider": {
                    "openrouter": {
                        "models": {
                            (slug): model_config
                        }
                    }
                }
            }),
        );
        return Ok(());
    }

    // DeepSeek (compte special b.ai et comptes DeepSeek) : l'intensite est
    // passee dans les options du modele (`reasoning_effort`, niveaux high/max
    // de DeepSeek V4) — c'est le seul canal que le CLI opencode applique pour
    // ce provider, sans variante dediee.
    if inference_provider.eq_ignore_ascii_case("deepseek") {
        let Some(effort) = effort else {
            return Ok(());
        };
        let slug = model
            .strip_prefix("deepseek/")
            .filter(|slug| !slug.is_empty())
            .ok_or_else(|| "Nom de modele DeepSeek invalide".to_string())?;
        merge_opencode_config_content(
            command,
            json!({
                "provider": {
                    "deepseek": {
                        "models": {
                            (slug): {
                                "options": { "reasoning_effort": effort }
                            }
                        }
                    }
                }
            }),
        );
        return Ok(());
    }

    Ok(())
}

fn configure_codex_app_connectors(
    command: &mut Command,
    connectors: Option<&[ChatAppConnector]>,
    app_write_approved: bool,
) {
    let Some(connectors) = connectors else {
        return;
    };

    // Un agent autonome ne voit aucune app par defaut. Chaque connecteur est
    // reactive pour ce processus uniquement ; le config.toml du compte reste
    // intact et les chats ordinaires conservent leur comportement historique.
    command
        .arg("--enable")
        .arg("apps")
        .arg("-c")
        .arg("apps._default.enabled=false");

    let approval_mode = if app_write_approved {
        // L'interface a deja obtenu une autorisation humaine, precise et a
        // usage unique. Le prochain tour peut donc executer cette tranche.
        "approve"
    } else {
        // Les lectures annotees read-only restent autonomes ; les outils qui
        // ecrivent doivent demander une autorisation et echouent ferme en mode
        // non interactif si le modele ne respecte pas le protocole autonome.
        "writes"
    };
    for connector in [ChatAppConnector::Gmail, ChatAppConnector::GoogleCalendar] {
        if !connectors.contains(&connector) {
            continue;
        }
        let prefix = format!("apps.{}", connector.config_id());
        for value in [
            format!("{prefix}.enabled=true"),
            format!("{prefix}.default_tools_enabled=true"),
            format!("{prefix}.destructive_enabled=false"),
            format!("{prefix}.default_tools_approval_mode=\"{approval_mode}\""),
        ] {
            command.arg("-c").arg(value);
        }
    }
}

fn environment_memory_instructions(memory: &str) -> String {
    format!(
        "Memoire partagee de cet environnement, definie par l'utilisateur dans Codex Switch Terminal.\n\
Utilise-la comme contexte durable dans cette conversation. Une demande explicite plus recente et les fichiers du projet restent prioritaires. N'invente pas de details absents et ne mentionne cette memoire que si c'est utile.\n\n\
<environment_memory>\n{}\n</environment_memory>",
        memory.trim()
    )
}

fn autonomous_agent_tool_instructions() -> &'static str {
    "Capacite native Codex Switch Terminal : des outils MCP permettent de creer un goal durable, d'ouvrir un autre chat, de piloter les agents autonomes et d'utiliser le compte Microsoft 365 de l'utilisateur depuis un chat normal. Quand l'utilisateur demande explicitement un goal, appelle `create_goal` avec son objectif exact ; OpenCode expose ce meme outil sous le nom namespace `cst_chat_create_goal`, qu'il faut donc appeler dans ce runtime. Cet outil cree un agent autonome persistant qui herite du compte, du modele et de l'environnement du chat. Quand l'utilisateur demande explicitement d'ouvrir, creer ou lancer un chat normal separe, appelle `create_chat` avec son message initial. Le nouveau chat herite du compte, du modele, de l'effort de raisonnement et de l'environnement courants ; un seul chat peut etre cree par tour. Quand l'utilisateur demande explicitement d'envoyer, poster ou lancer un message dans un chat deja ouvert, appelle `send_chat_message` avec le message exact ; `chat` absent ou « current » vise le chat courant, sinon passe l'identifiant de session d'un autre chat ouvert. Ce canal n'est actif que s'il a ete active explicitement : ne le mentionne jamais theoriquement et ne t'envoie jamais un message a toi-meme en boucle. Les mots de passe, codes de verification et donnees bancaires y sont refuses et doivent etre saisis par l'utilisateur. Quand l'utilisateur demande explicitement de creer, lancer, demarrer ou rendre autonome un nouvel agent, appelle `create_autonomous_agent` avec un objectif precis. Quand il demande explicitement de mettre en pause l'agent autonome lie a ce chat, appelle `pause_autonomous_agent` sans demander d'identifiant ; cette pause arrete le cycle courant et empeche toute nouvelle planification jusqu'a une reprise explicite depuis l'interface. Quand il demande explicitement de modifier l'agent autonome lie a ce chat (nom, objectif, role, mode, frequence, validation humaine ou tests), appelle `update_autonomous_agent` avec uniquement les champs a changer. Quand il demande explicitement au superviseur d'activer, produire ou relancer le compte rendu general qui compile les rapports non lus par priorite, appelle `activate_supervisor_general_report` sans demander d'identifiant ; cet outil fonctionne depuis n'importe quel chat. Quand il demande explicitement d'ajouter une meme regle durable a plusieurs agents deja actifs qui utilisent la review humaine, appelle `apply_autonomous_agent_policy` avec une instruction precise et verifiable ; cet outil ne depend pas de la cle du chat, reste limite au compte courant et cible par defaut uniquement le projet courant. Utilise la portee `account` seulement si l'utilisateur vise explicitement tous ses projets. Pour une politique de validation visuelle, passe `requireVisualEvidence: true`, exige une capture ou maquette fidele avant autorisation, une capture du rendu reel apres implementation et une comparaison explicite avec correction des ecarts significatifs. Pour une politique non visuelle, passe `requireVisualEvidence: false`. Ne demande jamais d'identifiant d'agent. Deduis les reglages non critiques et conserve les objectifs, roles, frequences et garde-fous existants. N'appelle pas ces outils pour une question theorique. Ne pretends jamais qu'une creation, une modification ou une mise en pause a reussi si l'appel correspondant n'a pas reussi. Cinq outils supplementaires ouvrent le compte Microsoft 365 lie a l'utilisateur connecte. Quand il demande de consulter, chercher ou resumer ses e-mails, appelle `list_outlook_messages`. Quand il demande son planning, ses rendez-vous ou une disponibilite, appelle `list_calendar_events` ; les horaires retournes sont en UTC, convertis-les avant de les presenter et n'annonce jamais une heure sans avoir verifie le fuseau. Quand il demande d'ecrire ou d'envoyer un e-mail, appelle `send_outlook_email` avec un message complet et pret a partir. Quand il demande de poser un rendez-vous, appelle `create_calendar_event` apres avoir verifie le creneau avec `list_calendar_events`. Quand il demande de deplacer ou de modifier un evenement, appelle `update_calendar_event` avec l'identifiant obtenu par `list_calendar_events`. Ces trois derniers outils NE FONT PARTIR NI N'ECRIVENT RIEN : ils affichent une carte que l'utilisateur doit confirmer dans la conversation. N'ecris donc jamais que l'e-mail est parti, que l'invitation est envoyee ou que l'agenda est a jour ; dis que la proposition attend sa validation. La boite et l'agenda sont ceux du compte connecte : ne demande jamais d'identifiant, de mot de passe ni de boite tierce, et n'invente aucune adresse de destinataire. Si l'utilisateur a lie plusieurs boites Microsoft, sa boite principale sert par defaut ; quand il precise laquelle utiliser, passe son adresse dans le champ `account`, en n'y mettant qu'une de ses propres adresses liees."
}

fn merge_turn_instructions(
    environment_memory: Option<&str>,
    tool_instructions: Option<&str>,
) -> Option<String> {
    let sections = [environment_memory, tool_instructions]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    (!sections.is_empty()).then(|| sections.join("\n\n"))
}

fn merge_codex_developer_instructions(account_home: &Path, turn_instructions: &str) -> String {
    let existing = std::fs::read_to_string(account_home.join("config.toml"))
        .ok()
        .and_then(|content| content.parse::<toml::Value>().ok())
        .and_then(|config| {
            config
                .get("developer_instructions")
                .and_then(toml::Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
        });
    match existing {
        Some(existing) => format!("{existing}\n\n{turn_instructions}"),
        None => turn_instructions.to_string(),
    }
}

fn selected_model(
    requested: Option<&str>,
    fallback: Option<&str>,
) -> Result<Option<String>, String> {
    let value = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()));
    let Some(value) = value else {
        return Ok(None);
    };
    if value.chars().count() > MAX_MODEL_CHARS
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return Err("Nom de modele invalide".to_string());
    }
    Ok(Some(value.to_string()))
}

fn selected_reasoning_effort(
    provider: Provider,
    inference_provider: Option<&str>,
    requested: Option<&str>,
    fallback: Option<&str>,
) -> Result<Option<String>, String> {
    let request = requested.map(str::trim).filter(|value| !value.is_empty());
    let value = match provider {
        // Codex : la requete prime, puis le defaut persiste du compte.
        Provider::Codex => {
            request.or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        }
        // Claude : l'intensite (`--effort`) vient UNIQUEMENT de la requete
        // explicite. Le champ `reasoning_effort` du compte porte un defaut Codex
        // herite ("medium") qui n'a jamais eu de sens pour Claude ; s'y rabattre
        // ferait tourner un compte Claude sous son defaut natif (high). Sans
        // requete explicite, on ne passe pas `--effort` et Claude Code applique
        // son propre defaut.
        Provider::Claude => request,
        // OpenRouter : la requete prime, puis l'intensite choisie dans Compte
        // devient le defaut backend. Sans l'une ni l'autre, aucune variante
        // n'est passee et OpenRouter reste en mode Automatique.
        Provider::OpenCode
            if inference_provider
                .is_some_and(|provider| provider.eq_ignore_ascii_case("openrouter")) =>
        {
            request.or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        }
        // DeepSeek (compte special b.ai et comptes DeepSeek) : l'intensite
        // demandee ou celle du compte est appliquee via les options du modele
        // dans la config opencode (`reasoning_effort`, high/max de DeepSeek V4).
        Provider::OpenCode
            if inference_provider
                .is_some_and(|provider| provider.eq_ignore_ascii_case("deepseek")) =>
        {
            request.or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        }
        Provider::OpenCode | Provider::Freebuff | Provider::Aihubmix => return Ok(None),
        // OpenAI-compatible : l'intensite est injectee dans le corps HTTP
        // (champ `reasoning_effort_field`). La requete prime, puis le defaut
        // persiste du compte.
        Provider::OpenAiCompatible => {
            request.or_else(|| fallback.map(str::trim).filter(|value| !value.is_empty()))
        }
    };
    let Some(value) = value else {
        return Ok(None);
    };
    if !settings::is_valid_reasoning_effort(value) {
        return Err(format!("Intensite de raisonnement invalide : {value}"));
    }
    Ok(Some(value.to_string()))
}

fn validate_session_id(provider: Provider, session_id: &str) -> Result<(), String> {
    let valid = match provider {
        Provider::Codex | Provider::Claude => Uuid::parse_str(session_id).is_ok(),
        Provider::OpenCode
        | Provider::Freebuff
        | Provider::Aihubmix
        | Provider::OpenAiCompatible => {
            let len = session_id.chars().count();
            (1..=160).contains(&len)
                && session_id.chars().all(|character| {
                    character.is_ascii_alphanumeric() || matches!(character, '_' | '-')
                })
        }
    };
    if valid {
        Ok(())
    } else {
        Err("Identifiant de conversation invalide".to_string())
    }
}

fn configure_environment(
    command: &mut Command,
    settings: &AppSettings,
    account: &AccountProfile,
    account_home: &Path,
    project_dir: Option<&Path>,
) {
    for (key, value) in account.provider.home_env(account_home) {
        command.env(key, value);
    }
    command.env("NO_COLOR", "1");
    // Ces plafonds sont opt-in : les gros noeuds gardent leur comportement
    // historique. Sur le profil conteneur 9 Gio, chaque agent et tous ses descendants
    // evitent de creer un pool de douze threads et des builds paralleles. Les
    // vingt chats restent actifs ; c'est le travail interne de chacun qui est
    // rendu plus frugal.
    if let Ok(heap_mib) = env::var("CST_CHAT_NODE_HEAP_MIB") {
        if let Ok(heap_mib) = heap_mib.trim().parse::<usize>() {
            if heap_mib > 0 {
                command.env("NODE_OPTIONS", format!("--max-old-space-size={heap_mib}"));
            }
        }
    }
    if let Ok(build_jobs) = env::var("CST_CHAT_BUILD_JOBS") {
        if let Ok(build_jobs) = build_jobs.trim().parse::<usize>() {
            if build_jobs > 0 {
                command
                    .env("CARGO_BUILD_JOBS", build_jobs.to_string())
                    .env("CMAKE_BUILD_PARALLEL_LEVEL", build_jobs.to_string())
                    .env("MAKEFLAGS", format!("-j{build_jobs}"))
                    .env("npm_config_jobs", build_jobs.to_string())
                    .env("RUST_TEST_THREADS", build_jobs.to_string());
            }
        }
    }
    if let Ok(worker_threads) = env::var("CST_CHAT_WORKER_THREADS") {
        if let Ok(worker_threads) = worker_threads.trim().parse::<usize>() {
            if worker_threads > 0 {
                let worker_threads = worker_threads.to_string();
                command
                    .env("GOMAXPROCS", &worker_threads)
                    .env("MKL_NUM_THREADS", &worker_threads)
                    .env("NUMEXPR_NUM_THREADS", &worker_threads)
                    .env("OMP_NUM_THREADS", &worker_threads)
                    .env("OPENBLAS_NUM_THREADS", &worker_threads)
                    .env("RAYON_NUM_THREADS", &worker_threads)
                    .env("TOKIO_WORKER_THREADS", &worker_threads)
                    .env("UV_THREADPOOL_SIZE", &worker_threads);
            }
        }
    }
    command.env("MALLOC_ARENA_MAX", "1");
    if let Some(project_dir) = project_dir {
        command.current_dir(project_dir);
        command.env("PWD", project_dir.to_string_lossy().to_string());
    }
    if settings.proxy_controls_enabled {
        if let Some(proxy) = account.proxy_id.as_ref().and_then(|id| {
            settings
                .proxies
                .iter()
                .find(|candidate| candidate.id == *id)
        }) {
            for key in [
                "HTTP_PROXY",
                "HTTPS_PROXY",
                "ALL_PROXY",
                "http_proxy",
                "https_proxy",
                "all_proxy",
            ] {
                command.env(key, proxy.proxy_url.clone());
            }
        }
    }
}

fn resolve_project_dir(
    account: &AccountProfile,
    requested: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let raw = requested
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            account
                .project_dir
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
        });
    let Some(raw) = raw else {
        return Ok(None);
    };
    let path = settings::expand_home(raw)?;
    if !path.is_dir() {
        return Err(format!("Dossier introuvable : {raw}"));
    }
    Ok(Some(path))
}

fn review_proof_workspace(
    provider: Provider,
    project_dir: Option<&Path>,
    filesystem_scope: ChatFilesystemScope,
) -> Result<Option<PathBuf>, String> {
    if filesystem_scope != ChatFilesystemScope::ReviewProofArtifacts || provider != Provider::Codex
    {
        return Ok(None);
    }
    let project_dir = project_dir.ok_or_else(|| {
        "Un dossier projet est requis pour produire une preuve visuelle de review".to_string()
    })?;
    let project_root = std::fs::canonicalize(project_dir)
        .map_err(|error| format!("Dossier projet de la review inaccessible : {error}"))?;
    let candidate = project_root.join(".codex-proof");
    std::fs::create_dir_all(&candidate)
        .map_err(|error| format!("Preparation de `.codex-proof` impossible : {error}"))?;
    let proof_root = std::fs::canonicalize(&candidate)
        .map_err(|error| format!("Dossier `.codex-proof` inaccessible : {error}"))?;
    if proof_root == project_root || !proof_root.starts_with(&project_root) || !proof_root.is_dir()
    {
        return Err("Le dossier `.codex-proof` sort du projet autorise".to_string());
    }
    Ok(Some(proof_root))
}

fn review_proof_workspace_instructions(project_root: &Path, proof_root: &Path) -> String {
    format!(
        "BAC A SABLE DE PREUVE DE REVIEW : le projet source a inspecter est `{}` et reste en lecture seule pendant ce tour. Le repertoire de travail `{}` est l'unique zone du projet dans laquelle les commandes peuvent ecrire. Enregistre les captures ou maquettes directement dans ce repertoire (ou l'un de ses sous-dossiers), sans tenter de modifier le parent. Dans AUTONOMOUS_REVIEW_EVIDENCE, exprime toujours le chemin relativement au projet sous la forme `.codex-proof/<fichier>`, meme si la commande de capture utilise seulement `<fichier>` depuis le repertoire courant.",
        project_root.display(),
        proof_root.display(),
    )
}

struct ResolvedProviderProgram {
    executable: PathBuf,
    managed_codex_package_root: Option<PathBuf>,
}

fn resolve_provider_program(
    raw: &str,
    provider: Provider,
) -> Result<ResolvedProviderProgram, String> {
    let wrapper = resolve_cli_program(raw).map_err(|error| {
        if provider == Provider::OpenCode {
            format!(
                "{error}. Installe OpenCode (`npm install -g opencode-ai`) puis redemarre l'application"
            )
        } else {
            error
        }
    })?;
    if provider == Provider::Codex {
        if let Some((executable, package_root)) = resolve_native_npm_codex(&wrapper) {
            return Ok(ResolvedProviderProgram {
                executable,
                managed_codex_package_root: Some(package_root),
            });
        }
    }
    if provider == Provider::OpenCode {
        if let Some(executable) = resolve_native_npm_opencode(&wrapper) {
            return Ok(ResolvedProviderProgram {
                executable,
                managed_codex_package_root: None,
            });
        }
    }
    Ok(ResolvedProviderProgram {
        executable: wrapper,
        managed_codex_package_root: None,
    })
}

/// Construit une commande directement executable pour un provider. Cette
/// entree est partagee avec l'index de discussions OpenCode afin qu'un shim npm
/// Windows soit resolu exactement comme lors d'un tour de chat.
pub(crate) fn resolved_provider_command(raw: &str, provider: Provider) -> Result<Command, String> {
    let program = resolve_provider_program(raw, provider)?;
    let mut command = Command::new(&program.executable);
    configure_resolved_program_environment(&mut command, &program);
    Ok(command)
}

fn configure_resolved_program_environment(
    command: &mut Command,
    program: &ResolvedProviderProgram,
) {
    let Some(package_root) = program.managed_codex_package_root.as_ref() else {
        return;
    };
    // Reproduit l'environnement utile du lanceur JS officiel. Le serveur gere
    // deja les signaux et l'arret de l'arbre ; conserver Node et cmd.exe entre
    // le serveur et le binaire Rust ne ferait qu'ajouter deux processus par
    // tour. Le chemin npm classique est le seul layout optimise ici ; toute
    // installation personnalisee conserve son lanceur historique.
    for key in [
        "CODEX_MANAGED_BY_NPM",
        "CODEX_MANAGED_BY_PNPM",
        "CODEX_MANAGED_BY_BUN",
    ] {
        command.env_remove(key);
    }
    command
        .env("CODEX_MANAGED_PACKAGE_ROOT", package_root)
        .env("CODEX_MANAGED_BY_NPM", "1");
}

#[cfg(windows)]
fn resolve_native_npm_codex(wrapper: &Path) -> Option<(PathBuf, PathBuf)> {
    let is_official_shim = wrapper
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("codex.cmd"));
    if !is_official_shim {
        return None;
    }

    let shim_directory = wrapper.parent()?;
    let package_root = shim_directory
        .join("node_modules")
        .join("@openai")
        .join("codex");
    if !package_root.join("bin").join("codex.js").is_file() {
        return None;
    }
    let (platform_package, target_triple) = match env::consts::ARCH {
        "x86_64" => ("codex-win32-x64", "x86_64-pc-windows-msvc"),
        "aarch64" => ("codex-win32-arm64", "aarch64-pc-windows-msvc"),
        _ => return None,
    };
    let relative_binary = Path::new("vendor")
        .join(target_triple)
        .join("bin")
        .join("codex.exe");
    let candidates = [
        package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join(&relative_binary),
        shim_directory
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join(&relative_binary),
        package_root.join(relative_binary),
    ];
    let executable = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())?;
    let canonical_root = std::fs::canonicalize(&package_root).unwrap_or(package_root);
    Some((executable, canonical_root))
}

#[cfg(not(windows))]
fn resolve_native_npm_codex(wrapper: &Path) -> Option<(PathBuf, PathBuf)> {
    // Le paquet npm Unix expose normalement `bin/codex` comme lien vers
    // `@openai/codex/bin/codex.js`. Ce lanceur Node ne fait ensuite que
    // resoudre le binaire Rust de la plateforme et attendre sa fin. Dans un
    // serveur multi-chat, conserver ce parent Node coute environ 50 Mio RSS
    // par tour sans apporter de fonctionnalite supplementaire : le serveur
    // gere deja les flux, les signaux et l'arret du processus.
    let is_official_entry = wrapper
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, "codex" | "codex.js"));
    if !is_official_entry {
        return None;
    }

    let entrypoint = std::fs::canonicalize(wrapper).ok()?;
    if entrypoint.file_name().and_then(|name| name.to_str()) != Some("codex.js") {
        return None;
    }
    let bin_directory = entrypoint.parent()?;
    if bin_directory.file_name().and_then(|name| name.to_str()) != Some("bin") {
        return None;
    }
    let package_root = bin_directory.parent()?.to_path_buf();
    if package_root.file_name().and_then(|name| name.to_str()) != Some("codex")
        || package_root
            .parent()?
            .file_name()
            .and_then(|name| name.to_str())
            != Some("@openai")
    {
        return None;
    }

    let (platform_package, target_triple) = match (env::consts::OS, env::consts::ARCH) {
        ("linux" | "android", "x86_64") => ("codex-linux-x64", "x86_64-unknown-linux-musl"),
        ("linux" | "android", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => ("codex-darwin-x64", "x86_64-apple-darwin"),
        ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin"),
        _ => return None,
    };
    let relative_binary = Path::new("vendor")
        .join(target_triple)
        .join("bin")
        .join("codex");
    let openai_directory = package_root.parent()?;
    let candidates = [
        package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join(&relative_binary),
        openai_directory
            .join(platform_package)
            .join(&relative_binary),
        package_root.join(relative_binary),
    ];
    let executable = candidates
        .into_iter()
        .find(|candidate| candidate.is_file())?;
    Some((executable, package_root))
}

/// Le paquet npm officiel expose un shim `opencode.cmd` vers son binaire natif.
/// Lancer directement l'exe evite de conserver `cmd.exe` comme parent du tour
/// et rend le flux JSON/les signaux identiques aux installations Scoop/Chocolatey.
#[cfg(windows)]
fn resolve_native_npm_opencode(wrapper: &Path) -> Option<PathBuf> {
    let is_official_shim = wrapper
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("opencode.cmd"));
    if !is_official_shim {
        return None;
    }
    let executable = wrapper
        .parent()?
        .join("node_modules")
        .join("opencode-ai")
        .join("bin")
        .join("opencode.exe");
    executable.is_file().then_some(executable)
}

#[cfg(not(windows))]
fn resolve_native_npm_opencode(_wrapper: &Path) -> Option<PathBuf> {
    None
}

fn resolve_cli_program(raw: &str) -> Result<PathBuf, String> {
    let value = raw.trim().trim_matches('"');
    if value.is_empty() {
        return Err("Commande du provider vide".to_string());
    }
    let direct = PathBuf::from(value);
    if direct.is_file() {
        return Ok(direct);
    }
    if direct.components().count() > 1 {
        return Err(format!("Commande introuvable : {value}"));
    }

    let extensions: &[&str] = if cfg!(windows) {
        &["exe", "cmd", "bat", "com", ""]
    } else {
        &[""]
    };
    if let Some(path) = env::var_os("PATH") {
        for directory in env::split_paths(&path) {
            for extension in extensions {
                let name = if extension.is_empty() {
                    value.to_string()
                } else {
                    format!("{value}.{extension}")
                };
                let candidate = directory.join(name);
                if candidate.is_file() {
                    return Ok(candidate);
                }
            }
        }
    }
    Err(format!("Commande introuvable : {value}"))
}

#[cfg(windows)]
fn hide_process_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x0800_0000);
}

#[cfg(not(windows))]
fn hide_process_window(command: &mut Command) {
    use std::os::unix::process::CommandExt;

    // Isole chaque tour dans son propre groupe. Les outils lances par Codex
    // (code-mode, shells, serveurs de test) peuvent autrement survivre a la
    // fin du processus principal et conserver leur memoire dans WSL.
    command.process_group(0);
}

#[cfg(windows)]
fn terminate_chat_process_tree(child: &mut Child) -> std::io::Result<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }

    // `codex.cmd` demarre via cmd.exe, puis Node/Codex peut lui-meme lancer une
    // commande PowerShell. `Child::kill` ne termine que le wrapper direct et
    // laisse alors l'outil bloque en arriere-plan. `/T` ferme tout cet arbre.
    let pid = child.id().to_string();
    let mut taskkill = Command::new("taskkill.exe");
    taskkill
        .args(["/PID", pid.as_str(), "/T", "/F"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_process_window(&mut taskkill);
    match taskkill.status() {
        Ok(status) if status.success() => Ok(()),
        _ if child.try_wait()?.is_some() => Ok(()),
        _ => child.kill(),
    }
}

#[cfg(not(windows))]
fn terminate_chat_process_tree(child: &mut Child) -> std::io::Result<()> {
    if child.try_wait()?.is_some() {
        Ok(())
    } else {
        let pid = child.id();
        if unix_process_group_is_isolated(pid) {
            let process_group = format!("-{pid}");
            if Command::new("kill")
                .args(["-KILL", "--", process_group.as_str()])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
            {
                return Ok(());
            }
        }
        child.kill()
    }
}

#[cfg(not(windows))]
fn unix_process_group_is_isolated(pid: u32) -> bool {
    Command::new("ps")
        .args(["-o", "pgid=", "-p", pid.to_string().as_str()])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| value.trim().parse::<u32>().ok())
        == Some(pid)
}

fn wait_for_child(turn: &Arc<ChatTurn>) -> Result<ExitStatus, String> {
    loop {
        let terminal_grace_elapsed = provider_terminal_event(turn)
            .is_some_and(|event| event.observed_at.elapsed() >= PROVIDER_EXIT_GRACE);
        let mut child_guard = turn
            .child
            .lock()
            .map_err(|_| "Processus du tour verrouillé".to_string())?;
        let child = child_guard
            .as_mut()
            .ok_or_else(|| "Processus du tour introuvable".to_string())?;
        let result = child.try_wait().map_err(|error| error.to_string())?;
        if let Some(status) = result {
            return Ok(status);
        }
        if terminal_grace_elapsed {
            // `turn.completed` / `result` est plus fiable que la survie d'un
            // wrapper CLI. Après un court délai de flush, termine le wrapper
            // resté vivant afin de libérer le tour et son workspace.
            terminate_chat_process_tree(child).map_err(|error| error.to_string())?;
            return child.wait().map_err(|error| error.to_string());
        }
        drop(child_guard);
        thread::sleep(Duration::from_millis(120));
    }
}

fn finish_turn(turn: &Arc<ChatTurn>, exit: Result<ExitStatus, String>, stderr: &str) {
    let provider_terminal = provider_terminal_event(turn);
    let Ok(mut snapshot) = turn.snapshot.lock() else {
        return;
    };
    let before = active_turn_signal_state(&snapshot);
    finish_turn_snapshot(&mut snapshot, provider_terminal, exit, stderr);
    let active_catalog_changed = before != active_turn_signal_state(&snapshot);
    drop(snapshot);
    if active_catalog_changed {
        turn.runtime_sync.notify(RuntimeSyncTopic::ActiveChatTurns);
    }
    archive_finished_snapshot(turn);
}

/// Remplace uniquement les allocations devenues immuables d'un tour termine
/// par leur representation zlib. Les metadonnees legeres restent disponibles
/// pour les catalogues actifs et l'eviction; `status` restitue le snapshot
/// original. Le flux stdout doit etre ferme afin qu'aucun evenement tardif ne
/// puisse diverger de l'archive.
fn archive_finished_snapshot(turn: &Arc<ChatTurn>) {
    if !turn.output_closed.load(Ordering::Acquire) {
        return;
    }
    let Ok(mut snapshot) = turn.snapshot.lock() else {
        return;
    };
    if matches!(
        snapshot.status,
        ChatTurnStatus::Running | ChatTurnStatus::Finalizing
    ) || snapshot.finished_at.is_none()
    {
        return;
    }
    let Ok(mut archived) = turn.archived_snapshot.lock() else {
        return;
    };
    if archived.is_some() {
        return;
    }

    let reclaimable_bytes = snapshot_reclaimable_heap_bytes(&snapshot);
    if reclaimable_bytes == 0 {
        return;
    }
    let Ok(serialized) = serde_json::to_vec(&*snapshot) else {
        return;
    };
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    if encoder.write_all(&serialized).is_err() {
        return;
    }
    let Ok(mut compressed) = encoder.finish() else {
        return;
    };
    compressed.shrink_to_fit();
    if compressed.capacity() >= reclaimable_bytes {
        return;
    }

    *archived = Some(compressed);
    snapshot.error = None;
    snapshot.activities = Vec::new();
    snapshot.thoughts = Vec::new();
    snapshot.parts = Vec::new();
}

fn decode_archived_snapshot(compressed: &[u8]) -> Result<ChatTurnSnapshot, String> {
    let mut decoder = ZlibDecoder::new(compressed);
    let mut serialized = Vec::new();
    decoder
        .read_to_end(&mut serialized)
        .map_err(|error| format!("Archive du tour illisible : {error}"))?;
    serde_json::from_slice(&serialized)
        .map_err(|error| format!("Snapshot du tour illisible : {error}"))
}

fn snapshot_reclaimable_heap_bytes(snapshot: &ChatTurnSnapshot) -> usize {
    let mut bytes = snapshot.error.as_ref().map_or(0, String::capacity);
    bytes = bytes.saturating_add(
        snapshot
            .activities
            .capacity()
            .saturating_mul(std::mem::size_of::<ChatActivity>()),
    );
    for activity in &snapshot.activities {
        bytes = bytes
            .saturating_add(activity.id.capacity())
            .saturating_add(activity.kind.capacity())
            .saturating_add(activity.label.capacity())
            .saturating_add(activity.detail.as_ref().map_or(0, String::capacity))
            .saturating_add(activity.status.capacity());
    }
    bytes = bytes.saturating_add(
        snapshot
            .thoughts
            .capacity()
            .saturating_mul(std::mem::size_of::<ChatThought>()),
    );
    for thought in &snapshot.thoughts {
        bytes = bytes
            .saturating_add(thought.id.capacity())
            .saturating_add(thought.kind.capacity())
            .saturating_add(thought.text.capacity())
            .saturating_add(thought.status.capacity());
    }
    bytes = bytes.saturating_add(
        snapshot
            .parts
            .capacity()
            .saturating_mul(std::mem::size_of::<ChatPart>()),
    );
    for part in &snapshot.parts {
        bytes = bytes
            .saturating_add(part.id.capacity())
            .saturating_add(part.kind.capacity())
            .saturating_add(part.status.capacity())
            .saturating_add(part.text.as_ref().map_or(0, String::capacity))
            .saturating_add(part.tool.as_ref().map_or(0, String::capacity))
            .saturating_add(part.title.as_ref().map_or(0, String::capacity))
            .saturating_add(part.subtitle.as_ref().map_or(0, String::capacity))
            .saturating_add(part.detail.as_ref().map_or(0, String::capacity))
            .saturating_add(part.output.as_ref().map_or(0, String::capacity));
    }
    bytes
}

fn finish_turn_snapshot(
    snapshot: &mut ChatTurnSnapshot,
    provider_terminal: Option<ProviderTerminalEvent>,
    exit: Result<ExitStatus, String>,
    stderr: &str,
) {
    if snapshot.status == ChatTurnStatus::Cancelled {
        return;
    }
    if snapshot.status == ChatTurnStatus::Finalizing {
        match provider_terminal.map(|event| event.outcome) {
            Some(ProviderTerminalOutcome::Failed(error)) => {
                snapshot.status = ChatTurnStatus::Failed;
                snapshot.error = Some(first_non_empty(
                    &Some(error),
                    stderr,
                    "Le provider a signalé un échec",
                ));
                complete_running_activities(snapshot, "error");
                complete_running_thoughts(snapshot, "error");
                complete_running_parts(snapshot, "error");
            }
            _ => {
                snapshot.status = ChatTurnStatus::Completed;
                snapshot.error = None;
                complete_running_activities(snapshot, "complete");
                complete_running_thoughts(snapshot, "complete");
                complete_running_parts(snapshot, "complete");
                remove_final_commentary(snapshot);
            }
        }
        return;
    }
    snapshot.finished_at.get_or_insert_with(metrics::now_ts);
    match exit {
        Ok(status) if status.success() => {
            snapshot.status = ChatTurnStatus::Completed;
            complete_running_activities(snapshot, "complete");
            complete_running_thoughts(snapshot, "complete");
            complete_running_parts(snapshot, "complete");
            remove_final_commentary(snapshot);
        }
        Ok(status) => {
            snapshot.status = ChatTurnStatus::Failed;
            let fallback = format!("Le provider s'est arrêté avec le code {:?}", status.code());
            snapshot.error = Some(first_non_empty(&snapshot.error, stderr, &fallback));
            complete_running_activities(snapshot, "error");
            complete_running_thoughts(snapshot, "error");
            complete_running_parts(snapshot, "error");
        }
        Err(error) => {
            snapshot.status = ChatTurnStatus::Failed;
            snapshot.error = Some(first_non_empty(&snapshot.error, stderr, &error));
            complete_running_activities(snapshot, "error");
            complete_running_thoughts(snapshot, "error");
            complete_running_parts(snapshot, "error");
        }
    }
}

fn first_non_empty(existing: &Option<String>, stderr: &str, fallback: &str) -> String {
    existing
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| (!stderr.trim().is_empty()).then_some(stderr.trim()))
        .unwrap_or(fallback)
        .chars()
        .take(1200)
        .collect()
}

fn complete_running_activities(snapshot: &mut ChatTurnSnapshot, status: &str) {
    for activity in &mut snapshot.activities {
        if activity.status == "running" || activity.status == "queued" {
            activity.status = status.to_string();
        }
    }
}

fn complete_running_thoughts(snapshot: &mut ChatTurnSnapshot, status: &str) {
    for thought in &mut snapshot.thoughts {
        if thought.status == "running" || thought.status == "queued" {
            thought.status = status.to_string();
        }
    }
}

fn complete_running_parts(snapshot: &mut ChatTurnSnapshot, status: &str) {
    for part in &mut snapshot.parts {
        if part.status == "running" || part.status == "queued" {
            part.status = status.to_string();
        }
    }
}

fn is_request_user_input_name(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    if normalized == "request_user_input" {
        return true;
    }
    normalized
        .strip_suffix("request_user_input")
        .and_then(|prefix| prefix.chars().last())
        .is_some_and(|separator| matches!(separator, '.' | ':' | '/' | '_'))
}

fn part_waits_for_user_input(part: &ChatPart) -> bool {
    if part.kind != "tool"
        || matches!(part.status.as_str(), "error" | "failed" | "cancelled")
        || part
            .output
            .as_deref()
            .is_some_and(|output| !output.trim().is_empty())
    {
        return false;
    }
    [
        part.tool.as_deref(),
        part.title.as_deref(),
        part.subtitle.as_deref(),
    ]
    .into_iter()
    .flatten()
    .any(is_request_user_input_name)
        || [part.detail.as_deref(), part.subtitle.as_deref()]
            .into_iter()
            .flatten()
            .any(|value| {
                let normalized = value.to_ascii_lowercase();
                normalized.contains("tools.") && normalized.contains("request_user_input")
            })
}

fn active_turn_signal_state(
    snapshot: &ChatTurnSnapshot,
) -> Option<(String, Option<String>, ChatTurnStatus, bool)> {
    matches!(
        snapshot.status,
        ChatTurnStatus::Running | ChatTurnStatus::Finalizing
    )
    .then(|| {
        (
            snapshot.account_id.clone(),
            snapshot.session_id.clone(),
            snapshot.status,
            snapshot.parts.iter().any(part_waits_for_user_input),
        )
    })
}

fn remove_final_commentary(snapshot: &mut ChatTurnSnapshot) {
    if let Some(index) = snapshot
        .thoughts
        .iter()
        .rposition(|thought| thought.kind == "commentary")
    {
        snapshot.thoughts.remove(index);
    }
}

fn provider_terminal_event(turn: &Arc<ChatTurn>) -> Option<ProviderTerminalEvent> {
    turn.provider_terminal
        .lock()
        .ok()
        .and_then(|event| event.clone())
}

fn mark_provider_terminal(
    turn: &Arc<ChatTurn>,
    snapshot: &mut ChatTurnSnapshot,
    outcome: ProviderTerminalOutcome,
) {
    if snapshot.status != ChatTurnStatus::Running {
        return;
    }

    snapshot.status = ChatTurnStatus::Finalizing;
    snapshot.finished_at = Some(metrics::now_ts());
    match &outcome {
        ProviderTerminalOutcome::Completed => {
            snapshot.error = None;
            complete_running_activities(snapshot, "complete");
            complete_running_thoughts(snapshot, "complete");
            complete_running_parts(snapshot, "complete");
            remove_final_commentary(snapshot);
        }
        ProviderTerminalOutcome::Failed(error) => {
            snapshot.error = Some(error.clone());
            complete_running_activities(snapshot, "error");
            complete_running_thoughts(snapshot, "error");
            complete_running_parts(snapshot, "error");
        }
    }

    if let Ok(mut terminal) = turn.provider_terminal.lock() {
        terminal.get_or_insert(ProviderTerminalEvent {
            outcome,
            observed_at: Instant::now(),
        });
    }
}

fn apply_provider_event(turn: &Arc<ChatTurn>, provider: Provider, line: &str) {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return;
    };
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let Ok(mut snapshot) = turn.snapshot.lock() else {
        return;
    };
    let before = active_turn_signal_state(&snapshot);
    apply_provider_event_to_snapshot(turn, provider, &mut snapshot, &value, event_type);
    let active_catalog_changed = before != active_turn_signal_state(&snapshot);
    drop(snapshot);
    if active_catalog_changed {
        turn.runtime_sync.notify(RuntimeSyncTopic::ActiveChatTurns);
    }
}

fn apply_provider_event_to_snapshot(
    turn: &Arc<ChatTurn>,
    provider: Provider,
    snapshot: &mut ChatTurnSnapshot,
    value: &Value,
    event_type: &str,
) {
    if provider == Provider::OpenCode {
        apply_opencode_event(turn, snapshot, value, event_type);
        return;
    }

    if event_type == "thread.started" {
        snapshot.session_id = value
            .get("thread_id")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        mark_agent_started(snapshot);
        return;
    }
    if provider == Provider::Claude && event_type == "system" {
        if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
            snapshot.session_id = Some(session_id.to_string());
            mark_agent_started(snapshot);
        }
        return;
    }
    if provider == Provider::Claude && event_type == "assistant" {
        if let Some(part) = claude_part_from_event(value) {
            upsert_part(snapshot, part);
        }
        if let Some(thought) = claude_commentary_from_event(value) {
            upsert_thought(snapshot, thought);
        }
        return;
    }
    if event_type == "turn.completed" {
        mark_provider_terminal(turn, snapshot, ProviderTerminalOutcome::Completed);
        return;
    }
    if event_type == "turn.failed" {
        let error = event_error(value)
            .unwrap_or_else(|| "Le provider a signalé l'échec du tour".to_string());
        mark_provider_terminal(turn, snapshot, ProviderTerminalOutcome::Failed(error));
        return;
    }
    if provider == Provider::Claude && event_type == "result" {
        if let Some(session_id) = value.get("session_id").and_then(Value::as_str) {
            snapshot.session_id = Some(session_id.to_string());
        }
        let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");
        let failed = value.get("is_error").and_then(Value::as_bool) == Some(true)
            || subtype.starts_with("error");
        let outcome = if failed {
            ProviderTerminalOutcome::Failed(
                event_error(value)
                    .or_else(|| {
                        value
                            .get("result")
                            .and_then(Value::as_str)
                            .filter(|text| !text.trim().is_empty())
                            .map(ToString::to_string)
                    })
                    .unwrap_or_else(|| "Claude a signalé l'échec du tour".to_string()),
            )
        } else {
            ProviderTerminalOutcome::Completed
        };
        mark_provider_terminal(turn, snapshot, outcome);
        return;
    }
    if event_type == "error" {
        let error = event_error(value);
        if error
            .as_deref()
            .is_some_and(is_terminal_provider_error_message)
        {
            mark_provider_terminal(
                turn,
                snapshot,
                ProviderTerminalOutcome::Failed(error.unwrap_or_default()),
            );
        } else {
            snapshot.error = error;
        }
        return;
    }
    if event_type != "item.started" && event_type != "item.completed" {
        return;
    }
    let Some(item) = value.get("item") else {
        return;
    };
    let completed = event_type == "item.completed";
    if let Some(part) = part_from_item(item, completed) {
        upsert_part(snapshot, part);
    }
    if let Some(thought) = thought_from_item(item, completed) {
        upsert_thought(snapshot, thought);
    }
    if let Some(activity) = activity_from_item(item, completed) {
        upsert_activity(snapshot, activity);
    }
}

fn apply_opencode_event(
    turn: &Arc<ChatTurn>,
    snapshot: &mut ChatTurnSnapshot,
    value: &Value,
    event_type: &str,
) {
    if let Some(session_id) = value
        .get("sessionID")
        .and_then(Value::as_str)
        .or_else(|| value.pointer("/part/sessionID").and_then(Value::as_str))
    {
        snapshot.session_id = Some(session_id.to_string());
    }
    mark_agent_started(snapshot);

    match event_type {
        "text" | "reasoning" => {
            let Some(part) = value.get("part") else {
                return;
            };
            let Some(text) = part.get("text").and_then(Value::as_str) else {
                return;
            };
            if text.trim().is_empty() {
                return;
            }
            let id = part
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or(event_type)
                .to_string();
            upsert_part(
                snapshot,
                ChatPart {
                    id: id.clone(),
                    kind: if event_type == "reasoning" {
                        "reasoning".to_string()
                    } else {
                        "text".to_string()
                    },
                    status: "complete".to_string(),
                    text: Some(if event_type == "reasoning" {
                        limit_text(text, MAX_THOUGHT_CHARS)
                    } else {
                        text.to_string()
                    }),
                    tool: None,
                    title: None,
                    subtitle: None,
                    detail: None,
                    output: None,
                },
            );
            if event_type == "reasoning" {
                upsert_thought(
                    snapshot,
                    ChatThought {
                        id,
                        kind: "reasoning".to_string(),
                        text: limit_text(text, MAX_THOUGHT_CHARS),
                        status: "complete".to_string(),
                    },
                );
            }
        }
        "tool_use" => {
            let Some(part) = value.get("part") else {
                return;
            };
            let state = part.get("state").unwrap_or(&Value::Null);
            let state_status = state
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("running");
            let status = match state_status {
                "completed" => "complete",
                "error" | "failed" => "error",
                _ => "running",
            };
            let tool = part.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let id = part
                .get("id")
                .or_else(|| part.get("callID"))
                .and_then(Value::as_str)
                .unwrap_or(tool)
                .to_string();
            let title = state
                .get("title")
                .and_then(Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or("Utilisation d'un outil")
                .to_string();
            let detail = state
                .get("input")
                .and_then(compact_json)
                .map(|value| limit_part_detail(&value));
            let output = state
                .get("output")
                .or_else(|| state.get("error"))
                .and_then(compact_json)
                .map(|value| limit_part_detail(&value));
            upsert_part(
                snapshot,
                ChatPart {
                    id: id.clone(),
                    kind: "tool".to_string(),
                    status: status.to_string(),
                    text: None,
                    tool: Some(tool.to_string()),
                    title: Some(title.clone()),
                    subtitle: Some(short_detail(tool)),
                    detail: detail.clone(),
                    output,
                },
            );
            upsert_activity(
                snapshot,
                ChatActivity {
                    id,
                    kind: "tool".to_string(),
                    label: title,
                    detail: detail.map(|value| short_detail(&value)),
                    status: status.to_string(),
                },
            );
        }
        "step_finish" => {
            complete_running_activities(snapshot, "complete");
            complete_running_thoughts(snapshot, "complete");
            complete_running_parts(snapshot, "complete");
        }
        "error" => {
            let error = event_error(value)
                .unwrap_or_else(|| "OpenCode a signale l'echec du tour".to_string());
            mark_provider_terminal(turn, snapshot, ProviderTerminalOutcome::Failed(error));
        }
        _ => {}
    }
}

fn part_status(item: &Value, completed: bool) -> &'static str {
    let failed = item
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "error"));
    if failed {
        "error"
    } else if completed {
        "complete"
    } else {
        "running"
    }
}

fn part_from_item(item: &Value, completed: bool) -> Option<ChatPart> {
    let item_kind = item.get("type").and_then(Value::as_str)?;
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(item_kind)
        .to_string();
    let status = part_status(item, completed).to_string();

    if matches!(item_kind, "agent_message" | "reasoning") {
        let text = safe_item_text(
            item,
            (item_kind == "reasoning").then_some(MAX_THOUGHT_CHARS),
        )?;
        return Some(ChatPart {
            id,
            kind: if item_kind == "reasoning" {
                "reasoning".to_string()
            } else {
                "text".to_string()
            },
            status,
            text: Some(text),
            tool: None,
            title: None,
            subtitle: None,
            detail: None,
            output: None,
        });
    }

    let (tool, title, subtitle, detail, output) = match item_kind {
        "command_execution" => {
            let command = item.get("command").and_then(Value::as_str);
            let output = first_item_string(item, &["aggregated_output", "output", "stdout"]);
            (
                "command",
                if completed {
                    "Commande executee"
                } else {
                    "Execution d'une commande"
                },
                command.map(short_detail),
                command.map(limit_part_detail),
                output.map(limit_part_detail),
            )
        }
        "file_change" => {
            let changes = item.get("changes");
            let count = changes.and_then(Value::as_array).map(Vec::len).unwrap_or(0);
            (
                "edit",
                if completed {
                    "Fichiers modifies"
                } else {
                    "Modification de fichiers"
                },
                Some(format!("{count} changement(s)")),
                changes
                    .and_then(compact_json)
                    .map(|value| limit_part_detail(&value)),
                None,
            )
        }
        "mcp_tool_call" => {
            let name = item
                .get("tool")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str);
            let detail = item
                .get("arguments")
                .or_else(|| item.get("input"))
                .and_then(compact_json)
                .map(|value| limit_part_detail(&value));
            let output = item
                .get("result")
                .or_else(|| item.get("output"))
                .or_else(|| item.get("error"))
                .and_then(compact_json)
                .map(|value| limit_part_detail(&value));
            (
                "tool",
                "Utilisation d'un outil",
                name.map(short_detail),
                detail,
                output,
            )
        }
        "web_search" => {
            let query = item.get("query").and_then(Value::as_str);
            (
                "search",
                "Recherche web",
                query.map(short_detail),
                query.map(limit_part_detail),
                None,
            )
        }
        "plan" => (
            "plan",
            "Mise a jour du plan",
            None,
            compact_json(item).map(|value| limit_part_detail(&value)),
            None,
        ),
        _ => return None,
    };

    Some(ChatPart {
        id,
        kind: "tool".to_string(),
        status,
        text: None,
        tool: Some(tool.to_string()),
        title: Some(title.to_string()),
        subtitle,
        detail,
        output,
    })
}

fn claude_part_from_event(value: &Value) -> Option<ChatPart> {
    let message = value.get("message")?;
    let text = safe_text_value(message.get("content")?, None)?;
    Some(ChatPart {
        id: message
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("claude-commentary")
            .to_string(),
        kind: "text".to_string(),
        status: "complete".to_string(),
        text: Some(text),
        tool: None,
        title: None,
        subtitle: None,
        detail: None,
        output: None,
    })
}

fn compact_json(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.to_string()),
        _ => serde_json::to_string_pretty(value).ok(),
    }
}

fn first_item_string<'a>(item: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter()
        .find_map(|key| item.get(*key).and_then(Value::as_str))
}

fn limit_part_detail(value: &str) -> String {
    let clipped = value
        .chars()
        .take(MAX_PART_DETAIL_CHARS)
        .collect::<String>();
    if value.chars().count() > MAX_PART_DETAIL_CHARS {
        format!("{clipped}...")
    } else {
        clipped
    }
}

fn limit_text(value: &str, max_chars: usize) -> String {
    let clipped = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        format!("{clipped}…")
    } else {
        clipped
    }
}

fn mark_agent_started(snapshot: &mut ChatTurnSnapshot) {
    if let Some(activity) = snapshot
        .activities
        .iter_mut()
        .find(|item| item.id == "agent-start")
    {
        activity.label = "Conversation démarrée".to_string();
        activity.status = "complete".to_string();
    }
}

fn event_error(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| error.pointer("/data/message").and_then(Value::as_str))
                .or_else(|| error.get("name").and_then(Value::as_str))
                .or_else(|| error.as_str())
        })
        .or_else(|| value.get("message").and_then(Value::as_str))
        .map(|message| message.chars().take(1200).collect())
}

pub(crate) fn is_quota_exhaustion_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    [
        "usage limit",
        "rate limit",
        "insufficient_quota",
        "insufficient quota",
        "too many requests",
        "quota exceeded",
        "quota exhausted",
        "no tokens left",
        "zero tokens remaining",
        "out of tokens",
    ]
    .iter()
    .any(|needle| normalized.contains(needle))
}

pub(crate) fn is_model_capacity_message(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("selected model is at capacity")
        || (normalized.contains("model")
            && (normalized.contains("at capacity") || normalized.contains("overloaded")))
}

fn is_terminal_provider_error_message(message: &str) -> bool {
    is_quota_exhaustion_message(message) || is_model_capacity_message(message)
}

fn thought_from_item(item: &Value, completed: bool) -> Option<ChatThought> {
    let kind = item.get("type").and_then(Value::as_str)?;
    if !matches!(kind, "agent_message" | "reasoning") {
        return None;
    }
    let failed = item
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "error"));
    let status = if failed {
        "error"
    } else if completed {
        "complete"
    } else {
        "running"
    };
    let text = safe_item_text(item, Some(MAX_THOUGHT_CHARS));
    if kind == "agent_message" && text.is_none() {
        return None;
    }
    let text = text.unwrap_or_else(|| "Analyse en cours…".to_string());
    let id = if kind == "reasoning" && text == "Analyse en cours…" {
        "agent-thinking".to_string()
    } else {
        item.get("id")
            .and_then(Value::as_str)
            .unwrap_or(kind)
            .to_string()
    };
    Some(ChatThought {
        id,
        kind: if kind == "reasoning" {
            "reasoning".to_string()
        } else {
            "commentary".to_string()
        },
        text,
        status: status.to_string(),
    })
}

fn claude_commentary_from_event(value: &Value) -> Option<ChatThought> {
    let message = value.get("message")?;
    let text = safe_text_value(message.get("content")?, Some(MAX_THOUGHT_CHARS))?;
    let id = message
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("claude-commentary")
        .to_string();
    Some(ChatThought {
        id,
        kind: "commentary".to_string(),
        text,
        status: "complete".to_string(),
    })
}

fn safe_item_text(item: &Value, max_chars: Option<usize>) -> Option<String> {
    ["text", "summary_text", "summary", "content"]
        .iter()
        .find_map(|key| {
            item.get(*key)
                .and_then(|value| safe_text_value(value, max_chars))
        })
}

fn safe_text_value(value: &Value, max_chars: Option<usize>) -> Option<String> {
    let fragments = match value {
        Value::String(text) => vec![text.to_string()],
        Value::Array(values) => values
            .iter()
            .filter_map(|value| match value {
                Value::String(text) => Some(text.to_string()),
                Value::Object(object) => object
                    .get("text")
                    .or_else(|| object.get("summary_text"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                _ => None,
            })
            .collect(),
        Value::Object(object) => object
            .get("text")
            .or_else(|| object.get("summary_text"))
            .and_then(Value::as_str)
            .map(|text| vec![text.to_string()])
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let combined = fragments
        .into_iter()
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if combined.is_empty() {
        return None;
    }
    Some(match max_chars {
        Some(max_chars) => limit_text(&combined, max_chars),
        None => combined,
    })
}

fn activity_from_item(item: &Value, completed: bool) -> Option<ChatActivity> {
    let kind = item.get("type").and_then(Value::as_str)?;
    if matches!(kind, "agent_message" | "reasoning") {
        return None;
    }
    let id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(kind)
        .to_string();
    let failed = item
        .get("status")
        .and_then(Value::as_str)
        .is_some_and(|status| matches!(status, "failed" | "error"));
    let status = if failed {
        "error"
    } else if completed {
        "complete"
    } else {
        "running"
    };
    let (ui_kind, label, detail) = match kind {
        "command_execution" => (
            "command",
            if completed {
                "Commande exécutée"
            } else {
                "Exécution d'une commande"
            },
            item.get("command")
                .and_then(Value::as_str)
                .map(short_detail),
        ),
        "file_change" => (
            "edit",
            if completed {
                "Fichiers modifiés"
            } else {
                "Modification de fichiers"
            },
            item.get("changes")
                .and_then(Value::as_array)
                .map(|changes| format!("{} changement(s)", changes.len())),
        ),
        "mcp_tool_call" => (
            "tool",
            "Utilisation d'un outil",
            item.get("tool")
                .or_else(|| item.get("name"))
                .and_then(Value::as_str)
                .map(short_detail),
        ),
        "web_search" => (
            "search",
            "Recherche web",
            item.get("query").and_then(Value::as_str).map(short_detail),
        ),
        "plan" => ("plan", "Mise à jour du plan", None),
        _ => return None,
    };
    Some(ChatActivity {
        id,
        kind: ui_kind.to_string(),
        label: label.to_string(),
        detail,
        status: status.to_string(),
    })
}

fn upsert_activity(snapshot: &mut ChatTurnSnapshot, activity: ChatActivity) {
    if let Some(existing) = snapshot
        .activities
        .iter_mut()
        .find(|item| item.id == activity.id)
    {
        *existing = activity;
        return;
    }
    if snapshot.activities.len() < MAX_ACTIVITIES {
        snapshot.activities.push(activity);
    }
}

fn upsert_thought(snapshot: &mut ChatTurnSnapshot, thought: ChatThought) {
    if thought.id != "agent-thinking" {
        if let Some(starter) = snapshot
            .thoughts
            .iter_mut()
            .find(|item| item.id == "agent-thinking" && item.status == "running")
        {
            starter.status = "complete".to_string();
        }
    }
    if let Some(existing) = snapshot
        .thoughts
        .iter_mut()
        .find(|item| item.id == thought.id)
    {
        *existing = thought;
        return;
    }
    if snapshot.thoughts.len() >= MAX_THOUGHTS {
        snapshot.thoughts.remove(0);
    }
    snapshot.thoughts.push(thought);
}

fn upsert_part(snapshot: &mut ChatTurnSnapshot, part: ChatPart) {
    if let Some(existing) = snapshot.parts.iter_mut().find(|item| item.id == part.id) {
        *existing = part;
        return;
    }
    if snapshot.parts.len() >= MAX_PARTS {
        snapshot.parts.remove(0);
    }
    snapshot.parts.push(part);
}

fn short_detail(value: &str) -> String {
    let flattened = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if flattened.chars().count() > 120 {
        format!("{}…", flattened.chars().take(117).collect::<String>())
    } else {
        flattened
    }
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::Codex => "Codex",
        Provider::Claude => "Claude",
        Provider::OpenCode => "OpenCode",
        Provider::Freebuff => "Freebuff",
        Provider::Aihubmix => "AIHubMix",
        Provider::OpenAiCompatible => "OpenAI Compatible",
    }
}

fn display_path(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_account(provider: Provider) -> AccountProfile {
        AccountProfile {
            id: "account".to_string(),
            label: "Compte test".to_string(),
            created_at: None,
            provider,
            inference_provider: None,
            codex_home: ".codex-test".to_string(),
            project_dir: None,
            proxy_id: None,
            startup_command: None,
            limits: Default::default(),
            bypass: true,
            model: Some("modele-par-defaut".to_string()),
            reasoning_effort: Some("medium".to_string()),
            fast_mode: false,
            base_url: None,
            reasoning_effort_field: None,
            completed_on: None,
            api_key: None,
        }
    }

    fn command_args(command: &Command) -> Vec<String> {
        command
            .get_args()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect()
    }

    fn command_env_value(command: &Command, name: &str) -> Option<String> {
        command.get_envs().find_map(|(candidate, value)| {
            (candidate == std::ffi::OsStr::new(name))
                .then(|| value.map(|value| value.to_string_lossy().to_string()))
                .flatten()
        })
    }

    fn test_turn() -> Arc<ChatTurn> {
        Arc::new(ChatTurn {
            snapshot: Mutex::new(ChatTurnSnapshot {
                id: 1,
                account_id: "account".to_string(),
                session_id: None,
                source_chat_key: None,
                status: ChatTurnStatus::Running,
                started_at: 0,
                finished_at: None,
                error: None,
                activities: Vec::new(),
                thoughts: Vec::new(),
                parts: Vec::new(),
            }),
            archived_snapshot: Mutex::new(None),
            output_closed: AtomicBool::new(false),
            child: Mutex::new(None),
            provider_terminal: Mutex::new(None),
            runtime_sync: RuntimeSync::default(),
            live_session: Weak::new(),
        })
    }

    #[test]
    fn active_turns_follow_the_environment_when_no_owner_is_recorded() {
        let manager = ChatTurnManager::default();
        let mine = test_turn();
        let started_elsewhere = test_turn();
        started_elsewhere.snapshot.lock().expect("snapshot").id = 2;
        {
            let mut turns = manager.turns.lock().expect("catalogue");
            turns.insert(1, mine);
            turns.insert(2, started_elsewhere);
        }
        manager.assign_owner(1, "user-a").expect("proprietaire");
        // Un tour lance au jeton administrateur, par un agent autonome ou par
        // une orchestration n'a que son environnement pour identite.
        manager.remember_turn_workspace(1, Some(Path::new("/env")));
        manager.remember_turn_workspace(2, Some(Path::new("/env")));

        let ids = |turns: Vec<ActiveChatTurnSummary>| {
            turns.into_iter().map(|turn| turn.id).collect::<Vec<_>>()
        };
        assert_eq!(
            ids(manager
                .active_visible_to("user-a", |path| path == "/env")
                .expect("catalogue actif")),
            vec![1, 2],
            "le meme utilisateur voit le tour partout, quel que soit l'appareil qui l'a lance",
        );
        assert_eq!(
            ids(manager
                .active_visible_to("user-a", |_| false)
                .expect("catalogue actif")),
            vec![1],
            "sans acces a l'environnement, seul son propre tour reste visible",
        );
        assert!(manager
            .active_visible_to("user-b", |_| false)
            .expect("catalogue actif")
            .is_empty());

        // `chat_turn_status` et `stop_chat_turn` suivent la meme regle.
        assert!(manager
            .is_visible_to(2, "user-b", |_| true)
            .expect("visibilite"));
        assert!(!manager
            .is_visible_to(2, "user-b", |_| false)
            .expect("visibilite"));
        assert!(manager
            .is_visible_to(1, "user-a", |_| false)
            .expect("visibilite"));
    }

    #[test]
    fn live_claude_key_pairs_account_and_session() {
        assert_eq!(live_claude_key("acc", "sess"), "acc\u{0}sess");
        // Le meme couple (compte, session) donne toujours la meme clef.
        assert_eq!(
            live_claude_key("acc", "sess"),
            live_claude_key("acc", "sess")
        );
        // Des ids reels distincts (jamais de `\0`) ne collisionnent pas.
        assert_ne!(
            live_claude_key("acc", "sess-1"),
            live_claude_key("acc", "sess-2")
        );
        assert_ne!(
            live_claude_key("acc-1", "sess"),
            live_claude_key("acc-2", "sess")
        );
    }

    #[test]
    fn live_claude_user_message_line_is_single_json_line() {
        let line = live_claude_user_message_line("Bonjour\nseconde ligne");
        assert!(line.ends_with('\n'));
        // Une seule ligne physique malgre le saut de ligne du prompt.
        assert_eq!(line.trim_end_matches('\n').lines().count(), 1);
        let value: Value = serde_json::from_str(line.trim_end()).expect("ligne JSON valide");
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"], "Bonjour\nseconde ligne");
    }

    #[test]
    fn live_claude_profile_changes_when_launch_flags_change() {
        let home = Path::new("/home/acc");
        let base = live_claude_profile(
            home,
            Some(Path::new("/proj")),
            ChatTurnMode::Build,
            Some("modele"),
            Some("high"),
            Some("instructions"),
        );
        // Meme entree => meme empreinte (reutilise le process).
        assert_eq!(
            base,
            live_claude_profile(
                home,
                Some(Path::new("/proj")),
                ChatTurnMode::Build,
                Some("modele"),
                Some("high"),
                Some("instructions"),
            )
        );
        // Un modele different => empreinte differente (force le relance).
        assert_ne!(
            base,
            live_claude_profile(
                home,
                Some(Path::new("/proj")),
                ChatTurnMode::Build,
                Some("autre-modele"),
                Some("high"),
                Some("instructions"),
            )
        );
        // Des instructions differentes => empreinte differente.
        assert_ne!(
            base,
            live_claude_profile(
                home,
                Some(Path::new("/proj")),
                ChatTurnMode::Build,
                Some("modele"),
                Some("high"),
                Some("autres instructions"),
            )
        );
    }

    #[test]
    fn foreground_shell_net_targets_run_in_background() {
        // Le filet nomme explicitement l'option a bannir pour etre efficace.
        assert!(CLAUDE_FOREGROUND_SHELL_INSTRUCTIONS.contains("run_in_background"));
        // Il s'insere avant les instructions d'environnement, apres la relecture.
        let merged = merge_turn_instructions(
            Some(RESPONSE_QUALITY_INSTRUCTIONS),
            Some(CLAUDE_FOREGROUND_SHELL_INSTRUCTIONS),
        )
        .expect("fusion non vide");
        assert!(merged.contains("run_in_background"));
        assert!(merged.contains("relecture"));
    }

    #[test]
    fn claude_result_line_is_the_turn_boundary() {
        assert!(claude_line_is_turn_boundary(
            r#"{"type":"result","subtype":"success","session_id":"s"}"#
        ));
        // Une interruption clot aussi le tour par un `result` (sous-type erreur).
        assert!(claude_line_is_turn_boundary(
            r#"{"type":"result","subtype":"error_during_execution","session_id":"s"}"#
        ));
        // Les evenements intermediaires ne sont pas des frontieres.
        assert!(!claude_line_is_turn_boundary(
            r#"{"type":"assistant","message":{"content":"salut"}}"#
        ));
        assert!(!claude_line_is_turn_boundary(
            r#"{"type":"system","session_id":"s"}"#
        ));
        assert!(!claude_line_is_turn_boundary("pas du json"));
    }

    #[test]
    fn seed_turn_snapshot_starts_running_with_agent_start_activity() {
        let account = test_account(Provider::Claude);
        let snapshot = seed_turn_snapshot(
            7,
            &account,
            Some("sess-1".to_string()),
            Some("/proj".to_string()),
            Some("chat-7".to_string()),
        );
        assert_eq!(snapshot.id, 7);
        assert_eq!(snapshot.source_chat_key.as_deref(), Some("chat-7"));
        assert_eq!(snapshot.session_id.as_deref(), Some("sess-1"));
        assert_eq!(snapshot.status, ChatTurnStatus::Running);
        assert!(snapshot
            .activities
            .iter()
            .any(|activity| activity.id == "agent-start" && activity.status == "running"));
    }

    #[test]
    fn app_server_token_usage_notification_exposes_context_pressure() {
        let notification = json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "0199a213-81c0-7800-8aa1-bbab2a035a53",
                "tokenUsage": {
                    "last": { "totalTokens": 82_400 },
                    "modelContextWindow": 100_000
                }
            }
        });
        let usage = context_usage_from_app_server_notification(&notification)
            .expect("context usage notification");
        assert_eq!(usage.used_tokens, 82_400);
        assert_eq!(usage.context_window, 100_000);
        assert_eq!(usage.used_percent, 80);
    }

    #[test]
    fn codex_events_capture_thread_and_tools() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"thread.started","thread_id":"0199a213-81c0-7800-8aa1-bbab2a035a53"}"#,
        );
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.started","item":{"id":"item_1","type":"command_execution","command":"npm test","status":"in_progress"}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"command_execution","command":"npm test","status":"completed"}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.completed","item":{"id":"reason_1","type":"reasoning","summary":[{"text":"Je vérifie les tests avant de conclure."}]}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.completed","item":{"id":"message_1","type":"agent_message","text":"Les tests sont terminés."}}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(
            snapshot.session_id.as_deref(),
            Some("0199a213-81c0-7800-8aa1-bbab2a035a53")
        );
        assert_eq!(snapshot.activities.len(), 1);
        assert_eq!(snapshot.activities[0].status, "complete");
        assert_eq!(snapshot.thoughts.len(), 2);
        assert_eq!(snapshot.thoughts[0].kind, "reasoning");
        assert_eq!(
            snapshot.thoughts[0].text,
            "Je vérifie les tests avant de conclure."
        );
        assert_eq!(snapshot.thoughts[1].kind, "commentary");
        assert_eq!(snapshot.parts.len(), 3);
        assert_eq!(snapshot.parts[0].kind, "tool");
        assert_eq!(snapshot.parts[0].status, "complete");
        assert_eq!(snapshot.parts[1].kind, "reasoning");
        assert_eq!(snapshot.parts[2].kind, "text");
    }

    #[test]
    fn final_responses_remain_complete_beyond_the_thought_limit() {
        let body = "const resultat = calculer(valeur);\n".repeat(180);
        let response = format!("```ts\n{body}```");
        assert!(response.chars().count() > MAX_THOUGHT_CHARS);
        assert_eq!(
            safe_text_value(&json!("    ligne imbriquée\n"), None).as_deref(),
            Some("    ligne imbriquée\n"),
            "l'indentation et la fin du texte font partie de la syntaxe"
        );
        assert_eq!(safe_text_value(&json!(" \n\t"), None), None);

        let codex_item = json!({
            "id": "message-long",
            "type": "agent_message",
            "text": response
        });
        let codex_part = part_from_item(&codex_item, true).expect("reponse Codex");
        assert_eq!(codex_part.text.as_deref(), Some(response.as_str()));
        assert!(codex_part
            .text
            .as_deref()
            .is_some_and(|text| text.ends_with("```")));
        let codex_commentary = thought_from_item(&codex_item, true).expect("resume Codex");
        assert_eq!(
            codex_commentary.text.chars().count(),
            MAX_THOUGHT_CHARS + 1,
            "seule la copie de commentaire interne reste bornee"
        );

        let claude_event = json!({
            "message": {
                "id": "message-long",
                "content": [{ "type": "text", "text": response }]
            }
        });
        let claude_part = claude_part_from_event(&claude_event).expect("reponse Claude");
        assert_eq!(claude_part.text.as_deref(), Some(response.as_str()));
        let claude_commentary = claude_commentary_from_event(&claude_event).expect("resume Claude");
        assert_eq!(
            claude_commentary.text.chars().count(),
            MAX_THOUGHT_CHARS + 1
        );

        let opencode = test_turn();
        let opencode_event = json!({
            "type": "text",
            "sessionID": "session-longue",
            "part": { "id": "message-long", "text": response }
        });
        apply_provider_event(
            &opencode,
            Provider::OpenCode,
            &serde_json::to_string(&opencode_event).unwrap(),
        );
        let snapshot = opencode.snapshot.lock().unwrap();
        assert_eq!(snapshot.parts[0].text.as_deref(), Some(response.as_str()));
    }

    #[test]
    fn finished_snapshots_are_compressed_and_restored_without_data_loss() {
        let manager = ChatTurnManager::default();
        let turn = test_turn();
        {
            let mut snapshot = turn.snapshot.lock().unwrap();
            snapshot.status = ChatTurnStatus::Completed;
            snapshot.finished_at = Some(42);
            snapshot.error = Some("diagnostic final conserve".to_string());
            let detail = [
                "commande: cargo test --all-targets\n",
                "résultat: validation réussie sans perte fonctionnelle\n",
                "sortie: 0123456789abcdef0123456789abcdef\n",
            ]
            .concat()
            .repeat(100);
            for index in 0..MAX_PARTS {
                snapshot.parts.push(ChatPart {
                    id: format!("part-{index}"),
                    kind: "tool".to_string(),
                    status: "complete".to_string(),
                    text: None,
                    tool: Some("shell_command".to_string()),
                    title: Some(format!("Validation {index}")),
                    subtitle: None,
                    detail: Some(detail.clone()),
                    output: Some(format!("Code de sortie 0 pour l'étape {index}")),
                });
            }
        }
        let expected = {
            let snapshot = turn.snapshot.lock().unwrap();
            serde_json::to_value(&*snapshot).unwrap()
        };
        let reclaimable = {
            let snapshot = turn.snapshot.lock().unwrap();
            snapshot_reclaimable_heap_bytes(&snapshot)
        };

        archive_finished_snapshot(&turn);
        assert!(turn.archived_snapshot.lock().unwrap().is_none());
        turn.output_closed.store(true, Ordering::Release);
        archive_finished_snapshot(&turn);

        let compressed = turn
            .archived_snapshot
            .lock()
            .unwrap()
            .as_ref()
            .expect("archive compressee")
            .len();
        assert!(compressed < reclaimable);
        assert!(turn.snapshot.lock().unwrap().parts.is_empty());
        manager.turns.lock().unwrap().insert(1, turn);
        let restored = manager.status(1).expect("snapshot restaure");
        assert_eq!(serde_json::to_value(restored).unwrap(), expected);
        println!(
            "snapshot_reclaimable_bytes={reclaimable} archived_bytes={compressed} retained_cap_saving_bytes={}",
            (reclaimable - compressed) * MAX_RETAINED_TURNS
        );
    }

    #[test]
    fn finished_snapshots_keep_the_live_form_when_compression_would_cost_more() {
        let turn = test_turn();
        {
            let mut snapshot = turn.snapshot.lock().unwrap();
            snapshot.status = ChatTurnStatus::Failed;
            snapshot.finished_at = Some(42);
            snapshot.error = Some("x".to_string());
        }
        turn.output_closed.store(true, Ordering::Release);
        archive_finished_snapshot(&turn);

        assert!(turn.archived_snapshot.lock().unwrap().is_none());
        assert_eq!(turn.snapshot.lock().unwrap().error.as_deref(), Some("x"));
    }

    #[test]
    fn every_turn_requests_a_silent_language_and_syntax_review() {
        for expected in [
            "relecture silencieuse",
            "grammaire",
            "syntaxe",
            "orthographe",
            "langue de l'utilisateur",
            "délimiteurs et blocs sont complets",
        ] {
            assert!(RESPONSE_QUALITY_INSTRUCTIONS.contains(expected));
        }
        let instructions = merge_turn_instructions(Some(RESPONSE_QUALITY_INSTRUCTIONS), None)
            .expect("instructions qualite");
        assert_eq!(instructions, RESPONSE_QUALITY_INSTRUCTIONS);
    }

    #[test]
    fn every_turn_knows_the_canonical_switch_environment_classification() {
        for expected in [
            "SSD/Samsung T7",
            "poste local pc-fixe",
            "développement",
            "VPS Microsoft Azure",
            "seule version stable/production",
            "jamais la copie SSD/locale",
        ] {
            assert!(SWITCH_ENVIRONMENT_INSTRUCTIONS.contains(expected));
        }
    }

    #[test]
    fn codex_terminal_event_stops_visible_work_before_process_exit() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.completed","item":{"id":"message_1","type":"agent_message","text":"La réponse est terminée."}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Finalizing);
        assert!(snapshot.finished_at.is_some());
        assert!(snapshot
            .parts
            .iter()
            .all(|part| part.status != "running" && part.status != "queued"));
        drop(snapshot);
        assert!(matches!(
            provider_terminal_event(&turn).map(|event| event.outcome),
            Some(ProviderTerminalOutcome::Completed)
        ));
        finish_turn(&turn, Err("wrapper resté vivant".to_string()), "");
        assert_eq!(
            turn.snapshot.lock().unwrap().status,
            ChatTurnStatus::Completed
        );
    }

    #[test]
    fn claude_result_event_stops_visible_work_and_preserves_failure() {
        let successful = test_turn();
        apply_provider_event(
            &successful,
            Provider::Claude,
            r#"{"type":"result","subtype":"success","is_error":false,"session_id":"claude-session"}"#,
        );
        let snapshot = successful.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Finalizing);
        assert_eq!(snapshot.session_id.as_deref(), Some("claude-session"));
        drop(snapshot);
        assert!(matches!(
            provider_terminal_event(&successful).map(|event| event.outcome),
            Some(ProviderTerminalOutcome::Completed)
        ));

        let failed = test_turn();
        apply_provider_event(
            &failed,
            Provider::Claude,
            r#"{"type":"result","subtype":"error_during_execution","is_error":true,"result":"quota dépassé"}"#,
        );
        let snapshot = failed.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Finalizing);
        assert_eq!(snapshot.error.as_deref(), Some("quota dépassé"));
        drop(snapshot);
        assert!(matches!(
            provider_terminal_event(&failed).map(|event| event.outcome),
            Some(ProviderTerminalOutcome::Failed(error)) if error == "quota dépassé"
        ));
        finish_turn(&failed, Err("wrapper resté vivant".to_string()), "");
        let snapshot = failed.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Failed);
        assert_eq!(snapshot.error.as_deref(), Some("quota dépassé"));
    }

    #[test]
    fn codex_quota_error_is_terminal_even_if_a_command_wrapper_stays_alive() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"error","message":"You've hit your usage limit. Try again later."}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Finalizing);
        assert!(snapshot
            .error
            .as_deref()
            .is_some_and(|error| error.contains("usage limit")));
        drop(snapshot);
        assert!(matches!(
            provider_terminal_event(&turn).map(|event| event.outcome),
            Some(ProviderTerminalOutcome::Failed(error)) if error.contains("usage limit")
        ));

        finish_turn(&turn, Err("wrapper reste vivant".to_string()), "");
        assert_eq!(turn.snapshot.lock().unwrap().status, ChatTurnStatus::Failed);
    }

    #[test]
    fn codex_model_capacity_error_is_terminal_for_an_immediate_retry() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"error","message":"Selected model is at capacity. Please try a different model."}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(snapshot.status, ChatTurnStatus::Finalizing);
        assert!(snapshot
            .error
            .as_deref()
            .is_some_and(is_model_capacity_message));
        drop(snapshot);
        assert!(matches!(
            provider_terminal_event(&turn).map(|event| event.outcome),
            Some(ProviderTerminalOutcome::Failed(error)) if is_model_capacity_message(&error)
        ));

        finish_turn(&turn, Err("wrapper reste vivant".to_string()), "");
        assert_eq!(turn.snapshot.lock().unwrap().status, ChatTurnStatus::Failed);
    }

    #[test]
    fn codex_command_applies_chat_model_and_effort_overrides() {
        let account = test_account(Provider::Codex);
        let mut command = Command::new("codex");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("gpt-chat-test"),
            Some("ultra"),
            None,
            false,
            None,
            None,
            None,
        );

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--model", "gpt-chat-test"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "model_reasoning_effort=\"ultra\""]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "hide_agent_reasoning=false"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-c", "show_raw_agent_reasoning=false"]));
        assert!(args.windows(2).any(|pair| pair == ["--enable", "memories"]));
    }

    #[test]
    fn codex_review_proof_scope_never_opens_the_parent_project() {
        let account = test_account(Provider::Codex);
        let mut command = Command::new("codex");
        configure_provider_command_with_images_and_scope(
            &mut command,
            &account,
            None,
            ChatTurnMode::Plan,
            None,
            None,
            None,
            false,
            Some("review visuelle"),
            None,
            None,
            &[],
            ChatFilesystemScope::ReviewProofArtifacts,
        );

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        for expected in [
            "sandbox_mode=\"workspace-write\"",
            "approval_policy=\"never\"",
            "sandbox_workspace_write.network_access=false",
        ] {
            assert!(args
                .windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == expected));
        }
        assert!(args.windows(2).any(|pair| pair == ["-C", "."]));
        assert!(!args.iter().any(|arg| arg == "sandbox_mode=\"read-only\""));
        assert!(!args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"));
    }

    #[test]
    fn codex_review_proof_workspace_is_created_inside_the_project() {
        let project =
            std::env::temp_dir().join(format!("cst-review-proof-scope-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&project).unwrap();

        let proof = review_proof_workspace(
            Provider::Codex,
            Some(&project),
            ChatFilesystemScope::ReviewProofArtifacts,
        )
        .unwrap()
        .expect("workspace de preuve Codex");
        let canonical_project = std::fs::canonicalize(&project).unwrap();
        assert_eq!(proof, canonical_project.join(".codex-proof"));
        assert!(proof.is_dir());

        let instructions = review_proof_workspace_instructions(&canonical_project, &proof);
        assert!(instructions.contains("reste en lecture seule"));
        assert!(instructions.contains("l'unique zone du projet"));
        assert!(instructions.contains(".codex-proof/<fichier>"));
        assert!(review_proof_workspace(
            Provider::Claude,
            Some(&project),
            ChatFilesystemScope::ReviewProofArtifacts,
        )
        .unwrap()
        .is_none());

        let _ = std::fs::remove_dir_all(project);
    }

    #[test]
    fn opencode_command_uses_json_session_agent_model_and_auto_mode() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("deepseek".to_string());
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            Some("ses_abc123"),
            ChatTurnMode::Build,
            Some("deepseek/deepseek-v4-pro"),
            None,
            None,
            false,
            Some("memoire de test"),
            None,
            None,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        for expected in [
            ["run", "--format"],
            ["--format", "json"],
            ["--session", "ses_abc123"],
            ["--agent", "build"],
            ["--model", "deepseek/deepseek-v4-pro"],
        ] {
            assert!(args.windows(2).any(|pair| pair == expected));
        }
        assert!(args.iter().any(|arg| arg == "--thinking"));
        assert!(args.iter().any(|arg| arg == "--auto"));
    }

    #[test]
    fn opencode_chat_gets_the_scoped_goal_mcp_config() {
        let account = test_account(Provider::OpenCode);
        let config = ChatModelToolServerConfig {
            url: "http://127.0.0.1:8080/mcp/chat-tools".to_string(),
            bearer_token: "secret-capability".to_string(),
        };
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("openrouter/stealth/ox-alpha"),
            None,
            None,
            false,
            Some(autonomous_agent_tool_instructions()),
            Some(&config),
            None,
        );

        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(arguments
            .iter()
            .any(|argument| argument == "openrouter/stealth/ox-alpha"));
        assert!(arguments
            .iter()
            .all(|argument| !argument.contains("secret-capability")));

        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<Vec<_>>();
        assert!(environment.iter().any(|(key, value)| {
            key == MCP_BEARER_ENV && value.as_deref() == Some("secret-capability")
        }));

        let raw_config = command
            .get_envs()
            .find_map(|(key, value)| {
                (key == OPENCODE_CONFIG_CONTENT_ENV)
                    .then(|| value.map(|value| value.to_string_lossy().to_string()))
                    .flatten()
            })
            .expect("OPENCODE_CONFIG_CONTENT absent");
        let document: Value = serde_json::from_str(&raw_config).unwrap();
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/type")
                .and_then(Value::as_str),
            Some("remote")
        );
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/url")
                .and_then(Value::as_str),
            Some("http://127.0.0.1:8080/mcp/chat-tools")
        );
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/enabled")
                .and_then(Value::as_bool),
            Some(true)
        );
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/oauth")
                .and_then(Value::as_bool),
            Some(false)
        );
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/timeout")
                .and_then(Value::as_u64),
            Some(30_000)
        );
        assert_eq!(
            document
                .pointer("/mcp/cst_chat/headers/Authorization")
                .and_then(Value::as_str),
            Some("Bearer {env:CST_CHAT_AUTONOMOUS_TOOL_TOKEN}")
        );
        assert!(!raw_config.contains("secret-capability"));
    }

    #[test]
    fn opencode_overlay_deep_merges_inherited_config_and_honors_explicit_removal() {
        let mut command = Command::new("opencode");
        merge_opencode_config_content_with_inherited(
            &mut command,
            json!({ "mcp": { "cst_chat": { "enabled": true } } }),
            || {
                Some(
                    json!({
                        "mcp": { "personal": { "enabled": true } },
                        "provider": { "openrouter": { "options": { "sort": "price" } } }
                    })
                    .to_string(),
                )
            },
        );
        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let document: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(document["mcp"]["personal"]["enabled"], true);
        assert_eq!(document["mcp"]["cst_chat"]["enabled"], true);
        assert_eq!(
            document["provider"]["openrouter"]["options"]["sort"],
            "price"
        );

        let mut removed = Command::new("opencode");
        removed.env_remove(OPENCODE_CONFIG_CONTENT_ENV);
        merge_opencode_config_content_with_inherited(
            &mut removed,
            json!({ "mcp": { "cst_chat": { "enabled": true } } }),
            || Some(json!({ "inherited": true }).to_string()),
        );
        let encoded = command_env_value(&removed, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let document: Value = serde_json::from_str(&encoded).unwrap();
        assert!(document.get("inherited").is_none());
        assert_eq!(document["mcp"]["cst_chat"]["enabled"], true);
    }

    #[test]
    fn pasted_images_are_validated_written_and_removed() {
        let attachment = ChatImageAttachmentRequest {
            name: "capture.png".to_string(),
            mime_type: "image/png".to_string(),
            data_base64: BASE64_STANDARD.encode(b"\x89PNG\r\n\x1a\nimage-test"),
        };
        let images = TemporaryChatImages::create(91, &[attachment]).unwrap();
        let path = images.paths()[0].clone();
        assert!(path.exists());
        assert_eq!(
            path.extension().and_then(|value| value.to_str()),
            Some("png")
        );
        drop(images);
        assert!(!path.exists());

        let invalid = ChatImageAttachmentRequest {
            name: "fausse.png".to_string(),
            mime_type: "image/png".to_string(),
            data_base64: BASE64_STANDARD.encode(b"pas une image"),
        };
        assert!(TemporaryChatImages::create(92, &[invalid]).is_err());
    }

    #[test]
    fn orphan_sweep_removes_only_files_past_the_age_threshold() {
        let directory = env::temp_dir().join(format!("cst-image-sweep-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let orphan = directory.join("turn-42-orphan.png");
        std::fs::write(&orphan, b"x").unwrap();

        // Seuil large : une image tout juste ecrite (tour en cours) est conservee.
        sweep_orphan_chat_images_in(&directory, Duration::from_secs(3600));
        assert!(orphan.exists());

        // Seuil nul : tout fichier (age >= 0) est considere orphelin et efface.
        sweep_orphan_chat_images_in(&directory, Duration::from_secs(0));
        assert!(!orphan.exists());

        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn codex_command_attaches_each_pasted_image() {
        let account = test_account(Provider::Codex);
        let paths = [
            PathBuf::from("capture-a.png"),
            PathBuf::from("capture-b.webp"),
        ];
        let mut command = Command::new("codex");
        configure_provider_command_with_images(
            &mut command,
            &account,
            Some("0199a213-81c0-7800-8aa1-bbab2a035a53"),
            ChatTurnMode::Build,
            None,
            None,
            None,
            false,
            None,
            None,
            None,
            &paths,
        );
        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--image", "capture-a.png"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--image", "capture-b.webp"]));
        let last_image = args.iter().rposition(|arg| arg == "--image").unwrap();
        let session = args
            .iter()
            .position(|arg| arg == "0199a213-81c0-7800-8aa1-bbab2a035a53")
            .unwrap();
        assert!(
            last_image < session,
            "les options image precedent l'identifiant de reprise"
        );
    }

    #[test]
    fn opencode_events_capture_session_text_reasoning_and_tools() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::OpenCode,
            r#"{"type":"reasoning","sessionID":"ses_test","part":{"id":"reason_1","type":"reasoning","text":"Je verifie."}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::OpenCode,
            r#"{"type":"tool_use","sessionID":"ses_test","part":{"id":"tool_1","type":"tool","tool":"bash","state":{"status":"completed","title":"Tests","input":{"command":"npm test"},"output":"ok"}}}"#,
        );
        apply_provider_event(
            &turn,
            Provider::OpenCode,
            r#"{"type":"text","sessionID":"ses_test","part":{"id":"text_1","type":"text","text":"Tout est valide."}}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(snapshot.session_id.as_deref(), Some("ses_test"));
        assert_eq!(snapshot.parts.len(), 3);
        assert_eq!(snapshot.parts[0].kind, "reasoning");
        assert_eq!(snapshot.parts[1].kind, "tool");
        assert_eq!(snapshot.parts[1].status, "complete");
        assert_eq!(snapshot.parts[2].text.as_deref(), Some("Tout est valide."));
        assert_eq!(snapshot.thoughts.len(), 1);
        assert_eq!(snapshot.activities.len(), 1);
    }

    #[test]
    fn provider_session_validation_accepts_opencode_ids_only_for_opencode() {
        assert!(validate_session_id(Provider::OpenCode, "ses_abc-123").is_ok());
        assert!(validate_session_id(Provider::Codex, "ses_abc-123").is_err());
        assert!(validate_session_id(Provider::OpenCode, "bad id").is_err());
    }

    #[test]
    fn codex_chat_gets_the_scoped_autonomous_agent_mcp_tool() {
        let _env_guard = crate::chat_model_tools::CHAT_POST_ENV_LOCK.lock().unwrap();
        let account = test_account(Provider::Codex);
        let config = ChatModelToolServerConfig {
            url: "http://127.0.0.1:8080/mcp/chat-tools".to_string(),
            bearer_token: "secret-capability".to_string(),
        };
        let mut command = Command::new("codex");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            None,
            None,
            None,
            false,
            Some(autonomous_agent_tool_instructions()),
            Some(&config),
            None,
        );

        let args = command
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        for expected in [
            "mcp_servers.cst_chat.url=\"http://127.0.0.1:8080/mcp/chat-tools\"",
            "mcp_servers.cst_chat.bearer_token_env_var=\"CST_CHAT_AUTONOMOUS_TOOL_TOKEN\"",
            "mcp_servers.cst_chat.enabled_tools=[\"create_autonomous_agent\",\"update_autonomous_agent\",\"pause_autonomous_agent\",\"activate_supervisor_general_report\",\"apply_autonomous_agent_policy\",\"create_chat\",\"list_outlook_messages\",\"list_calendar_events\",\"send_outlook_email\",\"create_calendar_event\",\"update_calendar_event\",\"list_tiktok_dm_campaigns\",\"list_tiktok_sender_accounts\",\"manage_tiktok_sender_login\",\"select_tiktok_sender_account\",\"prepare_tiktok_dm_campaign\",\"send_tiktok_dm_campaign\",\"list_tiktok_follower_extractions\",\"extract_tiktok_followers\"]",
            "mcp_servers.cst_chat.required=false",
            "mcp_servers.cst_chat.startup_timeout_sec=15",
            "mcp_servers.cst_chat.default_tools_approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.create_autonomous_agent.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.update_autonomous_agent.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.pause_autonomous_agent.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.activate_supervisor_general_report.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.apply_autonomous_agent_policy.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.create_chat.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.list_outlook_messages.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.list_calendar_events.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.send_outlook_email.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.create_calendar_event.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.update_calendar_event.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.list_tiktok_dm_campaigns.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.list_tiktok_sender_accounts.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.manage_tiktok_sender_login.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.select_tiktok_sender_account.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.prepare_tiktok_dm_campaign.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.send_tiktok_dm_campaign.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.list_tiktok_follower_extractions.approval_mode=\"approve\"",
            "mcp_servers.cst_chat.tools.extract_tiktok_followers.approval_mode=\"approve\"",
        ] {        assert!(args
            .windows(2)
            .any(|pair| pair[0] == "-c" && pair[1] == expected));
        }
        let environment = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().to_string(),
                    value.map(|value| value.to_string_lossy().to_string()),
                )
            })
            .collect::<Vec<_>>();
        assert!(environment.iter().any(|(key, value)| {
            key == MCP_BEARER_ENV && value.as_deref() == Some("secret-capability")
        }));
        assert!(autonomous_agent_tool_instructions().contains("appelle `create_goal`"));
        assert!(autonomous_agent_tool_instructions().contains("`cst_chat_create_goal`"));
        assert!(autonomous_agent_tool_instructions().contains("appelle `create_autonomous_agent`"));
        assert!(autonomous_agent_tool_instructions().contains("appelle `update_autonomous_agent`"));
        assert!(autonomous_agent_tool_instructions().contains("appelle `pause_autonomous_agent`"));
        assert!(autonomous_agent_tool_instructions()
            .contains("appelle `activate_supervisor_general_report`"));
        assert!(autonomous_agent_tool_instructions()
            .contains("appelle `apply_autonomous_agent_policy`"));
        assert!(autonomous_agent_tool_instructions().contains("appelle `create_chat`"));
        assert!(autonomous_agent_tool_instructions()
            .to_ascii_lowercase()
            .contains("ne demande jamais d'identifiant"));
        for tool in [
            "list_outlook_messages",
            "list_calendar_events",
            "send_outlook_email",
            "create_calendar_event",
            "update_calendar_event",
        ] {
            assert!(autonomous_agent_tool_instructions().contains(&format!("appelle `{tool}`")));
        }
        // Le garde-fou central : le modele ne doit jamais annoncer un envoi que
        // seule la confirmation humaine declenche reellement.
        assert!(autonomous_agent_tool_instructions().contains("NE FONT PARTIR NI N'ECRIVENT RIEN"));
        // Le canal d'envoi de message est documente dans les instructions mais
        // pas expose sans le flag explicite : il n'apparait pas dans la liste
        // enabled_tools ci-dessus, ni comme outil a approuver.
        assert!(autonomous_agent_tool_instructions().contains("appelle `send_chat_message`"));
        let enabled_tools_arg = args
            .iter()
            .find(|arg| arg.contains("enabled_tools"))
            .expect("la config MCP contient une liste enabled_tools");
        assert!(!enabled_tools_arg.contains(SEND_CHAT_MESSAGE_TOOL_NAME));
        assert!(!args.iter().any(|arg| {
            arg.contains("tools.")
                && arg.contains(SEND_CHAT_MESSAGE_TOOL_NAME)
                && arg.contains("approval_mode")
        }));
    }

    #[test]
    fn send_chat_message_is_enabled_only_with_the_explicit_flag() {
        let _env_guard = crate::chat_model_tools::CHAT_POST_ENV_LOCK.lock().unwrap();
        std::env::remove_var("CST_AGENT_CHAT_POST");
        assert!(!crate::chat_model_tools::chat_post_tool_enabled());
        std::env::set_var("CST_AGENT_CHAT_POST", "1");
        assert!(crate::chat_model_tools::chat_post_tool_enabled());
        std::env::remove_var("CST_AGENT_CHAT_POST");
    }

    #[test]
    fn autonomous_connectors_are_scoped_and_writes_need_a_one_turn_approval() {
        let account = test_account(Provider::Codex);
        let connectors = [ChatAppConnector::Gmail, ChatAppConnector::GoogleCalendar];
        let mut guarded = Command::new("codex");
        configure_provider_command(
            &mut guarded,
            &account,
            None,
            ChatTurnMode::Build,
            None,
            None,
            Some(&connectors),
            false,
            None,
            None,
            None,
        );
        let guarded_args = guarded
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(!guarded_args
            .iter()
            .any(|arg| arg == "--dangerously-bypass-approvals-and-sandbox"));
        for expected in [
            "apps._default.enabled=false",
            "apps.gmail.enabled=true",
            "apps.gmail.destructive_enabled=false",
            "apps.gmail.default_tools_approval_mode=\"writes\"",
            "apps.connector_googlecalendar.enabled=true",
            "apps.connector_googlecalendar.destructive_enabled=false",
            "apps.connector_googlecalendar.default_tools_approval_mode=\"writes\"",
        ] {
            assert!(guarded_args
                .windows(2)
                .any(|pair| pair[0] == "-c" && pair[1] == expected));
        }

        let mut approved = Command::new("codex");
        configure_provider_command(
            &mut approved,
            &account,
            None,
            ChatTurnMode::Build,
            None,
            None,
            Some(&[ChatAppConnector::Gmail]),
            true,
            None,
            None,
            None,
        );
        let approved_args = approved
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(approved_args
            .windows(2)
            .any(|pair| { pair == ["-c", "apps.gmail.default_tools_approval_mode=\"approve\""] }));
        assert!(!approved_args
            .iter()
            .any(|arg| arg.contains("connector_googlecalendar.enabled=true")));
    }

    #[test]
    fn environment_memory_is_injected_outside_the_visible_user_prompt() {
        let memory = environment_memory_instructions(
            "API publique en version 2.\nConserver les migrations SQLite.",
        );

        let codex_account = test_account(Provider::Codex);
        let mut codex = Command::new("codex");
        configure_provider_command(
            &mut codex,
            &codex_account,
            None,
            ChatTurnMode::Build,
            None,
            None,
            None,
            false,
            Some(&memory),
            None,
            None,
        );
        let codex_args = codex
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        let configured = codex_args
            .iter()
            .find_map(|arg| arg.strip_prefix("developer_instructions="))
            .expect("instructions developpeur Codex");
        let parsed = format!("value={configured}")
            .parse::<toml::Value>()
            .expect("chaine TOML valide");
        assert_eq!(
            parsed.get("value").and_then(toml::Value::as_str),
            Some(memory.as_str())
        );

        let claude_account = test_account(Provider::Claude);
        let mut claude = Command::new("claude");
        configure_provider_command(
            &mut claude,
            &claude_account,
            None,
            ChatTurnMode::Build,
            None,
            None,
            None,
            false,
            Some(&memory),
            None,
            None,
        );
        let claude_args = claude
            .get_args()
            .map(|arg| arg.to_string_lossy().to_string())
            .collect::<Vec<_>>();
        assert!(claude_args
            .windows(2)
            .any(|pair| pair[0] == "--append-system-prompt" && pair[1] == memory));
    }

    #[test]
    fn question_and_proof_tools_generate_independent_hidden_instructions() {
        assert_eq!(chat_tool_instructions(&[], false, false), None);

        let question = chat_tool_instructions(&[], true, false).expect("mode Question");
        assert!(question.contains("[Mode Question]"));
        assert!(question.contains("request_user_input"));
        assert!(!question.contains("[Mode Preuve]"));

        let proof = chat_tool_instructions(&[], false, true).expect("mode Preuve");
        assert!(proof.contains("[Mode Preuve]"));
        assert!(proof.contains("capture d'écran"));
        assert!(!proof.contains("[Mode Question]"));

        let combined = merge_turn_instructions(Some("memoire environnement"), Some(&proof))
            .expect("instructions combinees");
        assert!(combined.starts_with("memoire environnement\n\n"));
        assert!(combined.contains("[Mode Preuve]"));
    }

    #[test]
    fn selected_skills_use_a_temporary_file_outside_the_provider_command_line() {
        let path;
        {
            let file = TemporaryChatSkillsFile::create(42, "# Skill\n\nInstructions").unwrap();
            path = file.path.clone();
            assert_eq!(
                std::fs::read_to_string(&path).unwrap(),
                "# Skill\n\nInstructions"
            );
            let instructions = file.instructions();
            assert!(instructions.contains(&path.display().to_string()));
            assert!(!instructions.contains("# Skill"));
        }
        assert!(!path.exists());
    }

    #[test]
    fn environment_memory_keeps_existing_codex_developer_instructions() {
        let home = env::temp_dir().join(format!("cst-memory-config-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "developer_instructions = \"Toujours lancer les tests.\"\n",
        )
        .unwrap();

        let merged = merge_codex_developer_instructions(&home, "Memoire de Produit");
        assert!(merged.starts_with("Toujours lancer les tests."));
        assert!(merged.ends_with("Memoire de Produit"));
        let _ = std::fs::remove_dir_all(home);
    }

    #[test]
    fn thought_parser_never_falls_back_to_encrypted_reasoning() {
        let turn = test_turn();
        apply_provider_event(
            &turn,
            Provider::Codex,
            r#"{"type":"item.completed","item":{"id":"reason_1","type":"reasoning","summary":[],"encrypted_content":"private-payload"}}"#,
        );

        let snapshot = turn.snapshot.lock().unwrap();
        assert_eq!(snapshot.thoughts.len(), 1);
        assert_eq!(snapshot.thoughts[0].id, "agent-thinking");
        assert_eq!(snapshot.thoughts[0].text, "Analyse en cours…");
        assert!(!snapshot.thoughts[0].text.contains("private-payload"));
        assert!(
            snapshot.parts.is_empty(),
            "un raisonnement chiffre sans resume visible ne devient jamais une part"
        );
    }

    #[test]
    fn chat_overrides_are_validated_and_remain_backward_compatible() {
        let request: StartChatTurnRequest = serde_json::from_value(serde_json::json!({
            "accountId": "account",
            "prompt": "Bonjour"
        }))
        .unwrap();
        assert_eq!(request.model, None);
        assert_eq!(request.reasoning_effort, None);
        assert_eq!(request.app_connectors, None);
        assert!(!request.app_write_approved);
        assert!(request.agent_tools.is_empty());
        assert!(request.agent_skills.is_empty());
        assert!(!request.question_tool);
        assert!(!request.proof_tool);

        let enabled_tools: StartChatTurnRequest = serde_json::from_value(serde_json::json!({
            "accountId": "account",
            "prompt": "Bonjour",
            "agentTools": [
                "thermo-nuclear-code-quality-review",
                "impeccable-make-interfaces-feel-better"
            ],
            "agentSkills": [{
                "id": "impeccable",
                "name": "Impeccable",
                "content": "Inspecte l'interface."
            }],
            "questionTool": true,
            "proofTool": true
        }))
        .unwrap();
        assert!(enabled_tools.question_tool);
        assert!(enabled_tools.proof_tool);
        assert_eq!(
            enabled_tools.agent_tools,
            vec![
                ChatAgentTool::ThermoNuclearCodeQualityReview,
                ChatAgentTool::ImpeccableMakeInterfacesFeelBetter,
            ]
        );
        assert_eq!(enabled_tools.agent_skills[0].id, "impeccable");

        assert_eq!(
            selected_model(Some(" gpt-chat-test "), Some("fallback")).unwrap(),
            Some("gpt-chat-test".to_string())
        );
        assert!(selected_model(Some("modele invalide"), None).is_err());
        assert_eq!(
            selected_reasoning_effort(Provider::Codex, None, Some("max"), None).unwrap(),
            Some("max".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(Provider::Codex, None, Some("ultra"), None).unwrap(),
            Some("ultra".to_string())
        );
        assert!(
            selected_reasoning_effort(Provider::Codex, None, Some("ultra mode"), None).is_err()
        );
        // Claude : l'intensite explicite de la requete est appliquee (--effort).
        assert_eq!(
            selected_reasoning_effort(Provider::Claude, None, Some("high"), Some("medium"))
                .unwrap(),
            Some("high".to_string())
        );
        // Claude : le defaut Codex herite sur le compte est ignore (pas de
        // fallback), pour ne pas rabaisser un compte Claude sous son defaut natif.
        assert_eq!(
            selected_reasoning_effort(Provider::Claude, None, None, Some("medium")).unwrap(),
            None
        );
        // DeepSeek (b.ai / comptes DeepSeek) : l'intensite demandee ou celle du
        // compte est appliquee ; les autres fournisseurs OpenCode ne gerent pas
        // l'intensite.
        assert_eq!(
            selected_reasoning_effort(Provider::OpenCode, Some("deepseek"), Some("high"), None)
                .unwrap(),
            Some("high".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(Provider::OpenCode, Some("freebuff"), Some("high"), None)
                .unwrap(),
            None
        );
    }

    #[test]
    fn openrouter_reasoning_effort_uses_request_then_account_fallback_then_automatic() {
        assert_eq!(
            selected_reasoning_effort(
                Provider::OpenCode,
                Some("openrouter"),
                Some("high"),
                Some("medium")
            )
            .unwrap(),
            Some("high".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(Provider::OpenCode, Some("OpenRouter"), None, Some("medium"))
                .unwrap(),
            Some("medium".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(Provider::OpenCode, Some("openrouter"), None, None).unwrap(),
            None
        );
    }

    #[test]
    fn openrouter_command_overlay_registers_selected_model_and_exact_variant() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("openrouter".to_string());
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("openrouter/~z-ai/glm-latest"),
            Some("high"),
            None,
            false,
            None,
            None,
            None,
        );
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("openrouter/~z-ai/glm-latest"),
            Some("high"),
        )
        .unwrap();

        let arguments = command_args(&command);
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["--variant", "high"]));
        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            overlay["provider"]["openrouter"]["models"]["~z-ai/glm-latest"]["variants"]["high"]
                ["reasoning"]["effort"],
            "high"
        );
        assert!(!encoded.contains("reasoningEffort"));
        assert!(!encoded.contains("sk-or-"));
        assert!(!encoded.to_ascii_lowercase().contains("proxy"));
    }

    #[test]
    fn deepseek_reasoning_effort_falls_back_to_the_account_default() {
        assert_eq!(
            selected_reasoning_effort(
                Provider::OpenCode,
                Some("deepseek"),
                None,
                Some("max"),
            )
            .unwrap(),
            Some("max".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(
                Provider::OpenCode,
                Some("deepseek"),
                Some("high"),
                Some("max"),
            )
            .unwrap(),
            Some("high".to_string())
        );
        assert_eq!(
            selected_reasoning_effort(Provider::OpenCode, Some("deepseek"), None, None).unwrap(),
            None
        );
    }

    #[test]
    fn deepseek_command_overlay_sets_reasoning_effort_in_model_options() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("deepseek".to_string());
        let mut command = Command::new("opencode");
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("deepseek/deepseek-v4-flash"),
            Some("max"),
        )
        .unwrap();

        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            overlay["provider"]["deepseek"]["models"]["deepseek-v4-flash"]["options"]
                ["reasoning_effort"],
            "max"
        );
        // Aucune variante openrouter ni cle ni proxy dans l'overlay.
        assert!(encoded.contains("reasoning_effort"));
        assert!(!encoded.to_ascii_lowercase().contains("variant"));
        assert!(!encoded.contains("sk-"));
        assert!(!encoded.to_ascii_lowercase().contains("proxy"));

        // Sans effort, aucun overlay deepseek n'est pose (config inchangee).
        let mut command = Command::new("opencode");
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("deepseek/deepseek-v4-flash"),
            None,
        )
        .unwrap();
        assert!(command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).is_none());
    }

    #[test]
    fn openrouter_command_overlay_preserves_existing_mcp_config() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("openrouter".to_string());
        let config = ChatModelToolServerConfig {
            url: "http://127.0.0.1:8080/mcp/chat-tools".to_string(),
            bearer_token: "secret-capability".to_string(),
        };
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("openrouter/vendor/model"),
            Some("low"),
            None,
            false,
            None,
            Some(&config),
            None,
        );
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("openrouter/vendor/model"),
            Some("low"),
        )
        .unwrap();

        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            overlay.pointer("/mcp/cst_chat/url").and_then(Value::as_str),
            Some("http://127.0.0.1:8080/mcp/chat-tools")
        );
        assert_eq!(
            overlay["provider"]["openrouter"]["models"]["vendor/model"]["variants"]["low"]
                ["reasoning"]["effort"],
            "low"
        );
        assert!(!encoded.contains("secret-capability"));
    }

    #[test]
    fn opencode_mcp_overlay_preserves_openrouter_provider_when_applied_last() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("openrouter".to_string());
        let config = ChatModelToolServerConfig {
            url: "http://127.0.0.1:8080/mcp/chat-tools".to_string(),
            bearer_token: "secret-capability".to_string(),
        };
        let mut command = Command::new("opencode");
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("openrouter/vendor/model"),
            Some("high"),
        )
        .unwrap();
        configure_opencode_model_tool(&mut command, Some(&config));

        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            overlay.pointer("/mcp/cst_chat/url").and_then(Value::as_str),
            Some("http://127.0.0.1:8080/mcp/chat-tools")
        );
        assert_eq!(
            overlay["provider"]["openrouter"]["models"]["vendor/model"]["variants"]["high"]
                ["reasoning"]["effort"],
            "high"
        );
        assert!(!encoded.contains("secret-capability"));
    }

    #[test]
    fn openrouter_command_overlay_keeps_automatic_without_variant() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("openrouter".to_string());
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("openrouter/openai/gpt-latest"),
            None,
            None,
            false,
            None,
            None,
            None,
        );
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("openrouter/openai/gpt-latest"),
            None,
        )
        .unwrap();

        assert!(!command_args(&command)
            .iter()
            .any(|argument| argument == "--variant"));
        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        let model = &overlay["provider"]["openrouter"]["models"]["openai/gpt-latest"];
        assert!(model.is_object());
        assert!(model.get("variants").is_none());
    }

    #[test]
    fn openrouter_command_overlay_registers_explicit_none_variant() {
        let mut account = test_account(Provider::OpenCode);
        account.inference_provider = Some("openrouter".to_string());
        let mut command = Command::new("opencode");
        configure_provider_command(
            &mut command,
            &account,
            None,
            ChatTurnMode::Build,
            Some("openrouter/vendor/optional-reasoning"),
            Some("none"),
            None,
            false,
            None,
            None,
            None,
        );
        configure_opencode_model_overlay(
            &mut command,
            &account,
            Some("openrouter/vendor/optional-reasoning"),
            Some("none"),
        )
        .unwrap();

        let encoded = command_env_value(&command, OPENCODE_CONFIG_CONTENT_ENV).unwrap();
        let overlay: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(
            overlay["provider"]["openrouter"]["models"]["vendor/optional-reasoning"]["variants"]
                ["none"]["reasoning"]["effort"],
            "none"
        );
        assert!(command_args(&command)
            .windows(2)
            .any(|pair| pair == ["--variant", "none"]));
    }

    #[test]
    fn conversation_claim_closes_the_check_then_insert_race() {
        let manager = ChatTurnManager::default();
        let request = StartChatTurnRequest {
            account_id: "account".to_string(),
            session_id: Some("0199a213-81c0-7800-8aa1-bbab2a035a53".to_string()),
            prompt: "test".to_string(),
            image_attachments: Vec::new(),
            project_dir: None,
            mode: ChatTurnMode::Build,
            model: None,
            reasoning_effort: None,
            app_connectors: None,
            app_write_approved: false,
            agent_tools: Vec::new(),
            agent_skills: Vec::new(),
            question_tool: false,
            proof_tool: false,
            source_chat_key: None,
        };
        let claim = manager.reserve_turn(&request).unwrap();
        assert!(manager.reserve_turn(&request).is_err());
        drop(claim);
        assert!(manager.reserve_turn(&request).is_ok());
    }

    #[test]
    fn new_conversations_do_not_share_an_empty_provider_session_claim() {
        let manager = ChatTurnManager::default();
        let request = StartChatTurnRequest {
            account_id: "account".to_string(),
            session_id: None,
            prompt: "test".to_string(),
            image_attachments: Vec::new(),
            project_dir: None,
            mode: ChatTurnMode::Build,
            model: None,
            reasoning_effort: None,
            app_connectors: None,
            app_write_approved: false,
            agent_tools: Vec::new(),
            agent_skills: Vec::new(),
            question_tool: false,
            proof_tool: false,
            source_chat_key: None,
        };

        let first = manager.reserve_turn(&request).unwrap();
        let second = manager.reserve_turn(&request).unwrap();
        drop((first, second));

        // Reproduit le court intervalle observe au redemarrage : le premier
        // agent tourne deja, mais Codex n'a pas encore emis `thread.started`.
        manager.turns.lock().unwrap().insert(1, test_turn());
        assert!(manager.reserve_turn(&request).is_ok());
    }

    #[test]
    fn active_turn_catalog_only_returns_busy_snapshots() {
        let manager = ChatTurnManager::default();
        let running = test_turn();
        let finalizing = test_turn();
        {
            let mut snapshot = finalizing.snapshot.lock().unwrap();
            snapshot.id = 2;
            snapshot.status = ChatTurnStatus::Finalizing;
            snapshot.parts.push(ChatPart {
                id: "question".to_string(),
                kind: "tool".to_string(),
                status: "running".to_string(),
                text: None,
                tool: Some("tool".to_string()),
                title: Some("Utilisation d'un outil".to_string()),
                subtitle: Some("functions.request_user_input".to_string()),
                detail: None,
                output: None,
            });
        }
        let completed = test_turn();
        {
            let mut snapshot = completed.snapshot.lock().unwrap();
            snapshot.id = 3;
            snapshot.status = ChatTurnStatus::Completed;
            snapshot.finished_at = Some(1);
        }
        manager
            .turns
            .lock()
            .unwrap()
            .extend([(3, completed), (1, running), (2, finalizing)]);

        let active = manager.active().unwrap();
        assert_eq!(
            active
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert!(!active[0].waiting_for_user);
        assert!(active[1].waiting_for_user);
        assert_eq!(manager.active_count(), 2);
    }

    #[test]
    fn global_chat_capacity_accepts_twenty_and_rejects_the_twenty_first_atomically() {
        let manager = ChatTurnManager::with_max_active(20);
        for id in 1..=20 {
            let turn = test_turn();
            turn.snapshot.lock().unwrap().id = id;
            manager.turns.lock().unwrap().insert(id, turn);
        }

        let request = StartChatTurnRequest {
            account_id: "account".to_string(),
            session_id: None,
            prompt: "test".to_string(),
            image_attachments: Vec::new(),
            project_dir: None,
            mode: ChatTurnMode::Build,
            model: None,
            reasoning_effort: None,
            app_connectors: None,
            app_write_approved: false,
            agent_tools: Vec::new(),
            agent_skills: Vec::new(),
            question_tool: false,
            proof_tool: false,
            source_chat_key: None,
        };
        let error = manager.start(request).expect_err("capacity must reject");
        assert!(error.starts_with("capacite chats atteinte: 20/20"));
    }

    #[test]
    fn zero_chat_capacity_has_no_numeric_limit() {
        let manager = ChatTurnManager::with_max_active(0);
        assert_eq!(manager.max_active, 0);
        assert!(!(manager.max_active > 0 && 10_000 >= manager.max_active));
    }

    #[test]
    fn session_busy_tracks_running_and_finalizing_turns() {
        let manager = ChatTurnManager::default();
        let turn = test_turn();
        {
            let mut snapshot = turn.snapshot.lock().unwrap();
            snapshot.session_id = Some("session-active".to_string());
        }
        manager.turns.lock().unwrap().insert(1, turn.clone());

        assert!(manager
            .session_is_busy("account", "session-active")
            .unwrap());
        assert!(!manager
            .session_is_busy("other-account", "session-active")
            .unwrap());

        turn.snapshot.lock().unwrap().status = ChatTurnStatus::Finalizing;
        assert!(manager
            .session_is_busy("account", "session-active")
            .unwrap());

        turn.snapshot.lock().unwrap().status = ChatTurnStatus::Completed;
        assert!(!manager
            .session_is_busy("account", "session-active")
            .unwrap());
    }

    #[test]
    fn stopping_a_turn_cancels_its_running_command_card() {
        let manager = ChatTurnManager::default();
        let mut runtime_updates = manager.runtime_sync().subscribe();
        let turn = test_turn();
        turn.snapshot.lock().unwrap().parts.push(ChatPart {
            id: "command".to_string(),
            kind: "tool".to_string(),
            status: "running".to_string(),
            text: None,
            tool: Some("command".to_string()),
            title: Some("Execution d'une commande".to_string()),
            subtitle: None,
            detail: None,
            output: None,
        });
        manager.turns.lock().unwrap().insert(1, turn);

        let stopped = manager.stop(1).unwrap();
        assert_eq!(stopped.status, ChatTurnStatus::Cancelled);
        assert_eq!(stopped.parts[0].status, "cancelled");
        assert_eq!(
            runtime_updates.try_recv().unwrap().topic,
            RuntimeSyncTopic::ActiveChatTurns
        );
    }

    #[cfg(all(windows, any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn official_npm_codex_uses_its_native_binary_with_safe_fallbacks() {
        let root = env::temp_dir().join(format!("cst-native-codex-{}", Uuid::new_v4()));
        let shim = root.join("codex.cmd");
        let package_root = root.join("node_modules").join("@openai").join("codex");
        let (platform_package, target_triple) = if cfg!(target_arch = "aarch64") {
            ("codex-win32-arm64", "aarch64-pc-windows-msvc")
        } else {
            ("codex-win32-x64", "x86_64-pc-windows-msvc")
        };
        let native = package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex.exe");
        std::fs::create_dir_all(package_root.join("bin")).unwrap();
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        std::fs::write(
            package_root.join("bin").join("codex.js"),
            "// official shim\n",
        )
        .unwrap();
        std::fs::write(&native, b"").unwrap();

        let resolved = resolve_provider_program(shim.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(resolved.executable, native);
        assert_eq!(
            resolved.managed_codex_package_root,
            Some(std::fs::canonicalize(&package_root).unwrap())
        );

        let custom = root.join("codex-custom.cmd");
        std::fs::write(&custom, "@echo off\r\n").unwrap();
        let custom_resolved =
            resolve_provider_program(custom.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(custom_resolved.executable, custom);
        assert!(custom_resolved.managed_codex_package_root.is_none());

        std::fs::remove_file(&native).unwrap();
        let fallback = resolve_provider_program(shim.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(fallback.executable, shim);
        assert!(fallback.managed_codex_package_root.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(all(not(windows), any(target_arch = "x86_64", target_arch = "aarch64")))]
    #[test]
    fn official_unix_npm_codex_uses_its_native_binary_with_safe_fallbacks() {
        let root = env::temp_dir().join(format!("cst-native-codex-unix-{}", Uuid::new_v4()));
        let shim = root.join("bin").join("codex");
        let package_root = root
            .join("lib")
            .join("node_modules")
            .join("@openai")
            .join("codex");
        let entrypoint = package_root.join("bin").join("codex.js");
        let (platform_package, target_triple) = match (env::consts::OS, env::consts::ARCH) {
            ("linux" | "android", "aarch64") => ("codex-linux-arm64", "aarch64-unknown-linux-musl"),
            ("linux" | "android", _) => ("codex-linux-x64", "x86_64-unknown-linux-musl"),
            ("macos", "aarch64") => ("codex-darwin-arm64", "aarch64-apple-darwin"),
            ("macos", _) => ("codex-darwin-x64", "x86_64-apple-darwin"),
            _ => unreachable!("plateforme Unix de test non prise en charge"),
        };
        let native = package_root
            .join("node_modules")
            .join("@openai")
            .join(platform_package)
            .join("vendor")
            .join(target_triple)
            .join("bin")
            .join("codex");
        std::fs::create_dir_all(shim.parent().unwrap()).unwrap();
        std::fs::create_dir_all(entrypoint.parent().unwrap()).unwrap();
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&entrypoint, "#!/usr/bin/env node\n").unwrap();
        std::fs::write(&native, b"").unwrap();
        std::os::unix::fs::symlink(&entrypoint, &shim).unwrap();

        let resolved = resolve_provider_program(shim.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(resolved.executable, native);
        assert_eq!(
            resolved.managed_codex_package_root,
            Some(std::fs::canonicalize(&package_root).unwrap())
        );

        let direct_entrypoint =
            resolve_provider_program(entrypoint.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(direct_entrypoint.executable, native);
        assert_eq!(
            direct_entrypoint.managed_codex_package_root,
            Some(std::fs::canonicalize(&package_root).unwrap())
        );

        std::fs::remove_file(&native).unwrap();
        let fallback = resolve_provider_program(shim.to_str().unwrap(), Provider::Codex).unwrap();
        assert_eq!(fallback.executable, shim);
        assert!(fallback.managed_codex_package_root.is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn official_npm_opencode_uses_its_native_binary() {
        let root = env::temp_dir().join(format!("cst-native-opencode-{}", Uuid::new_v4()));
        let shim = root.join("opencode.cmd");
        let native = root
            .join("node_modules")
            .join("opencode-ai")
            .join("bin")
            .join("opencode.exe");
        std::fs::create_dir_all(native.parent().unwrap()).unwrap();
        std::fs::write(&shim, "@echo off\r\n").unwrap();
        std::fs::write(&native, b"").unwrap();

        let resolved =
            resolve_provider_program(shim.to_str().unwrap(), Provider::OpenCode).unwrap();
        assert_eq!(resolved.executable, native);
        assert!(resolved.managed_codex_package_root.is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn stopping_a_chat_turn_kills_its_descendant_command() {
        let marker = env::temp_dir().join(format!("cst-chat-tree-stop-{}.txt", Uuid::new_v4()));
        let escaped_marker = marker.to_string_lossy().replace('\'', "''");
        let command_line = format!(
            "powershell.exe -NoProfile -NonInteractive -Command \"Start-Sleep -Milliseconds 900; [IO.File]::WriteAllText('{escaped_marker}', 'survived')\""
        );
        let mut command = Command::new("cmd.exe");
        command
            .args(["/D", "/S", "/C", command_line.as_str()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_process_window(&mut command);
        let mut child = command.spawn().expect("wrapper de test");

        thread::sleep(Duration::from_millis(200));
        terminate_chat_process_tree(&mut child).expect("arret de l'arbre du tour");
        let _ = child.wait();
        thread::sleep(Duration::from_millis(1_000));

        assert!(
            !marker.exists(),
            "la commande descendante a survecu a l'arret du tour"
        );
        let _ = std::fs::remove_file(marker);
    }

    #[cfg(not(windows))]
    #[test]
    fn stopping_a_unix_chat_turn_kills_its_process_group() {
        let marker = env::temp_dir().join(format!("cst-chat-tree-stop-{}.txt", Uuid::new_v4()));
        let mut command = Command::new("sh");
        command
            .args([
                "-c",
                "sleep 1; printf survived > \"$1\"",
                "cst-chat-test",
                marker.to_string_lossy().as_ref(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        hide_process_window(&mut command);
        let mut child = command.spawn().expect("wrapper Unix de test");

        thread::sleep(Duration::from_millis(200));
        terminate_chat_process_tree(&mut child).expect("arret du groupe du tour");
        let _ = child.wait();
        thread::sleep(Duration::from_millis(1_000));

        assert!(
            !marker.exists(),
            "un descendant Unix a survecu a l'arret du tour"
        );
        let _ = std::fs::remove_file(marker);
    }
}
