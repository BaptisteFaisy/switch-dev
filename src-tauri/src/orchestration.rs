//! Chats orchestres multi-agents, isoles des conversations ordinaires.
//!
//! Chaque execution dispose d'un worktree Git pour l'orchestrateur et d'un
//! worktree par tache. Un travailleur ne peut etre accepte qu'apres avoir
//! soumis une preuve structuree, puis apres revue et validation reelle dans le
//! worktree de l'orchestrateur. Des orchestrateurs testeurs dedies (minimum un,
//! puis un par tranche de cinq workers) concoivent et executent un plan de tests
//! obligatoire. Le diff final n'est applique au projet source que si tout le
//! quorum a valide et si son HEAD et son etat n'ont pas change depuis le depart.

use crate::{
    autonomous::AutonomousAgentManager,
    chat::{ChatTurnManager, ChatTurnMode, ChatTurnSnapshot, ChatTurnStatus, StartChatTurnRequest},
    discussions, fs_util, metrics, settings,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsStr,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(feature = "desktop")]
use tauri::State;
use uuid::Uuid;

const STORE_VERSION: u32 = 10;
const MAX_OBJECTIVE_BYTES: usize = 64 * 1024;
const MAX_NAME_CHARS: usize = 120;
const MAX_TEST_COMMAND_CHARS: usize = 8_000;
const DEFAULT_TEST_TIMEOUT_SECONDS: u64 = 10 * 60;
const MIN_TEST_TIMEOUT_SECONDS: u64 = 5;
const MAX_TEST_TIMEOUT_SECONDS: u64 = 30 * 60;
const DEFAULT_WORKER_COUNT: u32 = 5;
const MIN_WORKER_COUNT: u32 = 1;
/// Le test produit reste volontairement borne a cinq workers. Cette limite peut
/// etre relevee sans migration de donnees quand le pool de comptes et
/// l'infrastructure seront prets ; la limite de securite absolue reste 1000.
const DEFAULT_WORKER_LIMIT: u32 = 5;
const MAX_WORKER_COUNT: u32 = 1_000;
/// Un orchestrateur testeur controle au plus cinq missions. Le minimum reste
/// toujours un testeur, y compris pour une orchestration a un seul worker.
const WORKERS_PER_TESTER: u32 = 5;
const WORKER_COUNT_ENV: &str = "CST_ORCHESTRATION_WORKERS";
const WORKER_LIMIT_ENV: &str = "CST_ORCHESTRATION_MAX_WORKERS";
const DEFAULT_MAX_CONCURRENCY: u32 = 8;
const MIN_MAX_CONCURRENCY: u32 = 1;
const MAX_MAX_CONCURRENCY: u32 = 1_000;
/// Budget global de tours d'agents en vol a travers toutes les orchestrations.
/// Chaque tour en vol est un agent qui code vraiment (et consomme de la
/// memoire) ; les runs au-dela de ce budget attendent leur tour. C'est le
/// reglage qui pilote la memoire du SSD quand on deploye 200 agents :
/// `CST_ORCHESTRATION_CONCURRENCY` (defaut 8, borne 1..=200).
const CONCURRENCY_ENV: &str = "CST_ORCHESTRATION_CONCURRENCY";
/// Repertoire des sandboxes ephemeres des agents (worktrees Git, clones, patches,
/// handoffs). Par defaut a cote de l'etat persiste sur le SSD ; peut etre
/// redirige vers un RAM-disk (tmpfs) pour accelerer les fichiers temporaires
/// des agents : `CST_ORCHESTRATION_SANDBOX_DIR` (ex. /dev/shm/cst-orchestrated-runs).
const SANDBOX_DIR_ENV: &str = "CST_ORCHESTRATION_SANDBOX_DIR";
const DEFAULT_MAX_TASK_COUNT: u32 = DEFAULT_WORKER_LIMIT;
const MIN_TASK_COUNT: u32 = 1;
const MAX_TASK_COUNT: u32 = MAX_WORKER_COUNT;
/// Nombre de threads de pilotage asynchrones. Chaque driver parcourt les runs
/// actifs et les fait progresser en parallele : plusieurs agents peuvent ainsi
/// avancer sur le meme travail sans etre serialises par une boucle unique.
const DRIVER_COUNT_ENV: &str = "CST_ORCHESTRATION_DRIVERS";
const DEFAULT_DRIVER_COUNT: usize = 4;
const MIN_DRIVER_COUNT: usize = 1;
const MAX_DRIVER_COUNT: usize = 64;
const MAX_EVENTS: usize = 1_000;
const MAX_TEAM_MESSAGES: usize = 500;
const MAX_TEAM_MESSAGES_PER_TURN: usize = 20;
const MAX_TEAM_MESSAGE_CHARS: usize = 2_000;
const MAX_REVIEWS: usize = 20;
const MAX_PROTOCOL_FAILURES: u32 = 3;
const MAX_START_FAILURES: u32 = 3;
const MAX_TEXT_CHARS: usize = 12_000;
const MAX_TEST_OUTPUT_BYTES: usize = 96 * 1024;
const DELETE_QUIESCE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationStatus {
    Active,
    Paused,
    Completed,
    NeedsAttention,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationSourceKind {
    GitClean,
    GitDirty,
    Ephemeral,
}

impl Default for OrchestrationSourceKind {
    fn default() -> Self {
        Self::GitClean
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationPhase {
    Planning,
    DesigningTests,
    Working,
    Reviewing,
    Validating,
    Testing,
    Merging,
    FinalReview,
    FinalValidation,
    Publishing,
    Completed,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationTaskStatus {
    Pending,
    Working,
    Submitted,
    Reviewing,
    Validating,
    RevisionRequested,
    Accepted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationTurnKind {
    Plan,
    TesterPlan,
    Worker,
    Review,
    TesterValidation,
    MergeReview,
    FinalReview,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationTesterStatus {
    Pending,
    Designing,
    Ready,
    Testing,
    Passed,
    RevisionRequired,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationReviewDecision {
    Accept,
    Revise,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationValidationKind {
    Task,
    Final,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationEvent {
    pub timestamp: i64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationTeamMessage {
    pub id: String,
    pub sequence: u64,
    pub timestamp: i64,
    pub from_role: OrchestrationAccountRole,
    #[serde(default)]
    pub from_task_id: Option<String>,
    #[serde(default)]
    pub to_task_ids: Vec<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationProofTest {
    pub command: String,
    pub result: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationProof {
    pub summary: String,
    pub files_changed: Vec<String>,
    pub tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub risks: Vec<String>,
    pub submitted_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationReview {
    pub decision: OrchestrationReviewDecision,
    pub summary: String,
    pub feedback: String,
    #[serde(default)]
    pub tests: Vec<OrchestrationProofTest>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationTestDefinition {
    pub name: String,
    pub command: String,
    pub expected: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationTester {
    pub id: String,
    pub position: u32,
    pub status: OrchestrationTesterStatus,
    #[serde(default)]
    pub assigned_task_ids: Vec<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub plan_summary: Option<String>,
    #[serde(default)]
    pub test_plan: Vec<OrchestrationTestDefinition>,
    #[serde(default)]
    pub last_results: Vec<OrchestrationProofTest>,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub protocol_failures: u32,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationTask {
    pub id: String,
    pub position: u32,
    pub title: String,
    pub description: String,
    #[serde(default)]
    pub acceptance_criteria: Vec<String>,
    pub status: OrchestrationTaskStatus,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub handoff_pending: bool,
    #[serde(default)]
    pub handoff_count: u32,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub workspace_dir: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub workspace_generation: u32,
    #[serde(default)]
    pub attempt_count: u32,
    #[serde(default)]
    pub protocol_failures: u32,
    #[serde(default)]
    pub evidence: Option<OrchestrationProof>,
    #[serde(default)]
    pub reviews: Vec<OrchestrationReview>,
    #[serde(default)]
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OrchestrationSnapshot {
    pub id: String,
    pub name: String,
    pub objective: String,
    #[serde(default)]
    pub worker_count: u32,
    #[serde(default)]
    pub tester_count: u32,
    #[serde(default = "default_adaptive_fanout")]
    pub adaptive_fanout: bool,
    #[serde(default = "default_max_task_count")]
    pub max_task_count: u32,
    /// Plancher immuable choisi a la creation. Un routeur qui demande 20, 100
    /// ou 200 sous-agents ne peut pas etre silencieusement reduit par le planner.
    #[serde(default)]
    pub minimum_task_count: u32,
    #[serde(default = "default_max_concurrency")]
    pub max_concurrency: u32,
    pub account_id: String,
    #[serde(default)]
    pub orchestrator_account_id: String,
    #[serde(default)]
    pub worker_account_ids: Vec<String>,
    #[serde(default)]
    pub orchestrator_handoff_pending: bool,
    #[serde(default)]
    pub orchestrator_handoff_count: u32,
    /// Proprietaire HTTP injecte par le serveur. `None` preserve le mode
    /// desktop local et la migration des anciens snapshots.
    #[serde(default)]
    pub owner_id: Option<String>,
    #[serde(default)]
    pub source_kind: OrchestrationSourceKind,
    /// Chemin demande, conserve pour l'affichage uniquement.
    #[serde(default)]
    pub requested_project_dir: Option<String>,
    /// Frontiere ACL canonique. Elle ne doit jamais etre remplacee par le
    /// depot Git prive cree dans le sandbox.
    #[serde(default)]
    pub access_project_dir: Option<String>,
    pub project_dir: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    pub test_command: String,
    pub test_timeout_seconds: u64,
    pub status: OrchestrationStatus,
    pub phase: OrchestrationPhase,
    pub created_at: i64,
    pub updated_at: i64,
    pub base_commit: String,
    pub integrated_commit: String,
    pub sandbox_root: String,
    pub orchestrator_dir: String,
    #[serde(default)]
    pub orchestrator_session_id: Option<String>,
    #[serde(default)]
    pub current_turn_id: Option<u64>,
    #[serde(default)]
    pub current_turn_kind: Option<OrchestrationTurnKind>,
    #[serde(default)]
    pub current_task_id: Option<String>,
    #[serde(default)]
    pub current_tester_id: Option<String>,
    #[serde(default)]
    pub current_start_id: Option<String>,
    #[serde(default)]
    pub current_validation_id: Option<String>,
    #[serde(default)]
    pub current_validation_kind: Option<OrchestrationValidationKind>,
    #[serde(default)]
    pub next_action_at: Option<i64>,
    #[serde(default)]
    pub plan_summary: Option<String>,
    #[serde(default)]
    pub tasks: Vec<OrchestrationTask>,
    #[serde(default)]
    pub testers: Vec<OrchestrationTester>,
    /// Missions deja controlees ensemble par le chat orchestrateur apres le
    /// passage de leur testeur. Une mission rouverte est retiree de cette liste
    /// afin que sa nouvelle contribution repasse obligatoirement par la fusion.
    #[serde(default)]
    pub merge_reviewed_task_ids: Vec<String>,
    #[serde(default)]
    pub final_summary: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub consecutive_start_failures: u32,
    #[serde(default)]
    pub protocol_failures: u32,
    #[serde(default)]
    pub publish_applied: bool,
    /// Fil de coordination asynchrone borne. Les agents ne partagent ni
    /// processus ni contexte implicite : le serveur injecte ces messages dans
    /// leurs prochains tours, ce qui reste fiable avec une file de 1000 chats.
    #[serde(default)]
    pub team_messages: Vec<OrchestrationTeamMessage>,
    #[serde(default)]
    pub events: Vec<OrchestrationEvent>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateOrchestrationRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub objective: String,
    #[serde(default)]
    pub worker_count: Option<u32>,
    #[serde(default)]
    pub adaptive_fanout: Option<bool>,
    #[serde(default)]
    pub max_task_count: Option<u32>,
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    /// Session d'un chat normal a reprendre comme orchestrateur. Absente lors
    /// de la creation depuis la vue dediee.
    #[serde(default)]
    pub orchestrator_session_id: Option<String>,
    #[serde(default)]
    pub orchestrator_account_id: Option<String>,
    #[serde(default)]
    pub worker_account_ids: Vec<String>,
    #[serde(default)]
    pub account_id: String,
    #[serde(skip)]
    pub owner_id: Option<String>,
    #[serde(skip)]
    pub context_project_dir: Option<String>,
    #[serde(skip)]
    pub access_project_dir: Option<String>,
    pub project_dir: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    pub test_command: String,
    #[serde(default)]
    pub test_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromoteAutonomousAgentRequest {
    #[serde(default)]
    pub name: Option<String>,
    pub objective: String,
    #[serde(default)]
    pub worker_count: Option<u32>,
    #[serde(default)]
    pub adaptive_fanout: Option<bool>,
    #[serde(default)]
    pub max_task_count: Option<u32>,
    #[serde(default)]
    pub max_concurrency: Option<u32>,
    #[serde(default)]
    pub worker_account_ids: Vec<String>,
    #[serde(skip)]
    pub owner_id: Option<String>,
    #[serde(skip)]
    pub access_project_dir: Option<String>,
    pub project_dir: String,
    pub test_command: String,
    #[serde(default)]
    pub test_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum OrchestrationAction {
    Pause,
    Resume,
    Retry,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlOrchestrationRequest {
    pub action: OrchestrationAction,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OrchestrationAccountRole {
    Orchestrator,
    Worker,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReassignOrchestrationAccountRequest {
    pub role: OrchestrationAccountRole,
    #[serde(default)]
    pub worker_index: Option<u32>,
    pub account_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct OrchestrationStore {
    version: u32,
    #[serde(default)]
    runs: Vec<OrchestrationSnapshot>,
}

impl Default for OrchestrationStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            runs: Vec::new(),
        }
    }
}

#[derive(Clone)]
pub struct OrchestrationManager {
    inner: Arc<OrchestrationInner>,
}

struct OrchestrationInner {
    chat: ChatTurnManager,
    storage_path: PathBuf,
    sandboxes_path: PathBuf,
    store: Mutex<OrchestrationStore>,
    validation_runs: Mutex<HashMap<String, Arc<ValidationRun>>>,
    lifecycle: Mutex<()>,
    /// Verrou par run pour le pilotage asynchrone : chaque driver traite un run
    /// sous ce verrou, donc deux threads ne pilotent jamais le meme run en meme
    /// temps, tandis que des runs differents progressent en parallele.
    run_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

struct ValidationRun {
    id: String,
    cancelled: AtomicBool,
}

struct ValidationResult {
    passed: bool,
    exit_code: Option<i32>,
    duration_ms: u64,
    output: String,
}

struct PreparedAccountHandoff {
    session_id: Option<String>,
    handoff_pending: bool,
    summary: String,
}

struct PreparedSourceRepository {
    repository: PathBuf,
    base_commit: String,
    source_kind: OrchestrationSourceKind,
    requested_project_dir: Option<String>,
    access_project_dir: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanEnvelope {
    summary: String,
    tasks: Vec<PlanTask>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PlanTask {
    title: String,
    description: String,
    #[serde(default)]
    acceptance_criteria: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TesterPlanEnvelope {
    summary: String,
    tests: Vec<OrchestrationTestDefinition>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TesterResultEnvelope {
    decision: OrchestrationTesterDecision,
    summary: String,
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    feedback: String,
    tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProofEnvelope {
    summary: String,
    #[serde(default)]
    files_changed: Vec<String>,
    tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    risks: Vec<String>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReviewEnvelope {
    decision: OrchestrationReviewDecision,
    summary: String,
    #[serde(default)]
    feedback: String,
    #[serde(default)]
    tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeReviewEnvelope {
    decision: OrchestrationReviewDecision,
    summary: String,
    #[serde(default)]
    task_ids: Vec<String>,
    #[serde(default)]
    feedback: String,
    #[serde(default)]
    tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct FinalEnvelope {
    decision: FinalDecision,
    summary: String,
    #[serde(default)]
    task_id: Option<String>,
    #[serde(default)]
    feedback: String,
    #[serde(default)]
    tests: Vec<OrchestrationProofTest>,
    #[serde(default)]
    messages: Vec<TeamMessageEnvelope>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TeamMessageEnvelope {
    #[serde(default)]
    to_task_ids: Vec<String>,
    body: String,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum FinalDecision {
    Complete,
    Revise,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum OrchestrationTesterDecision {
    Pass,
    Revise,
}

enum WorkItem {
    Drive {
        run_id: String,
    },
    Poll {
        run_id: String,
        turn_id: u64,
        kind: OrchestrationTurnKind,
    },
}

impl WorkItem {
    fn run_id(&self) -> &str {
        match self {
            WorkItem::Drive { run_id } | WorkItem::Poll { run_id, .. } => run_id,
        }
    }
}

impl OrchestrationManager {
    pub fn new(chat: ChatTurnManager, storage_path: PathBuf) -> Result<Self, String> {
        let sandboxes_path = configured_sandboxes_path(&storage_path);
        fs::create_dir_all(&sandboxes_path).map_err(|error| error.to_string())?;
        let mut store = load_store(&storage_path)?;
        let recovered = normalize_loaded_store(&mut store, metrics::now_ts());
        if recovered {
            persist_store(&storage_path, &store)?;
        }
        let inner = Arc::new(OrchestrationInner {
            chat,
            storage_path,
            sandboxes_path,
            store: Mutex::new(store),
            validation_runs: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(()),
            run_locks: Mutex::new(HashMap::new()),
        });
        spawn_drivers(Arc::downgrade(&inner));
        Ok(Self { inner })
    }

    pub fn list(&self) -> Result<Vec<OrchestrationSnapshot>, String> {
        let mut runs = self
            .inner
            .store
            .lock()
            .map_err(|_| "Etat des orchestrations verrouille".to_string())?
            .runs
            .clone();
        runs.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        Ok(runs)
    }

    pub fn create(
        &self,
        request: CreateOrchestrationRequest,
    ) -> Result<OrchestrationSnapshot, String> {
        self.create_internal(request, false)
    }

    fn create_internal(
        &self,
        request: CreateOrchestrationRequest,
        start_paused: bool,
    ) -> Result<OrchestrationSnapshot, String> {
        let objective = validate_required_text(
            &request.objective,
            MAX_OBJECTIVE_BYTES,
            "L'objectif orchestre",
        )?;
        let name = validate_name(request.name.as_deref(), &objective)?;
        let worker_count = validate_worker_count(
            request
                .worker_count
                .unwrap_or_else(configured_default_worker_count),
        )?;
        let adaptive_fanout = request.adaptive_fanout.unwrap_or(true);
        let max_task_count = validate_max_task_count(
            request
                .max_task_count
                .unwrap_or_else(configured_worker_limit),
        )?;
        let minimum_task_count =
            requested_minimum_task_count(&objective, worker_count, adaptive_fanout, max_task_count);
        let max_concurrency = validate_max_concurrency(
            request
                .max_concurrency
                .unwrap_or_else(configured_default_max_concurrency),
        )?;
        if worker_count > max_task_count {
            return Err(format!(
                "Le plafond de taches ({max_task_count}) doit couvrir les {worker_count} workers demandes"
            ));
        }
        if adaptive_fanout
            && !adaptive_fanout_cardinalities(max_task_count)
                .iter()
                .any(|count| *count >= minimum_task_count)
        {
            return Err(format!(
                "Aucune cardinalite adaptative autorisee ne peut respecter le plancher de {minimum_task_count} avec un plafond de {max_task_count}"
            ));
        }
        let orchestrator_session_id = normalize_optional(request.orchestrator_session_id);
        if let Some(session_id) = orchestrator_session_id.as_deref() {
            Uuid::parse_str(session_id)
                .map_err(|_| "Identifiant du chat orchestrateur invalide".to_string())?;
        }
        let legacy_account_id = request.account_id.trim().to_string();
        let orchestrator_account_id = normalize_optional(request.orchestrator_account_id)
            .unwrap_or_else(|| legacy_account_id.clone());
        if orchestrator_account_id.is_empty() {
            return Err("Compte obligatoire pour l'orchestration".to_string());
        }
        let worker_account_ids = normalize_worker_accounts(
            request.worker_account_ids,
            worker_count,
            &orchestrator_account_id,
        )?;
        let app_settings = settings::load_settings_for_terminal()?;
        require_authenticated_account(&app_settings, &orchestrator_account_id)?;
        for worker_account_id in &worker_account_ids {
            require_authenticated_account(&app_settings, worker_account_id)?;
        }
        let requested_test_command = validate_required_text(
            &request.test_command,
            MAX_TEST_COMMAND_CHARS,
            "La commande de validation",
        )?;
        let test_timeout_seconds = validate_test_timeout(
            request
                .test_timeout_seconds
                .unwrap_or(DEFAULT_TEST_TIMEOUT_SECONDS),
        )?;
        let id = Uuid::new_v4().to_string();
        let sandbox_root = self.inner.sandboxes_path.join(&id);
        let orchestrator_dir = sandbox_root.join("orchestrator");
        fs::create_dir_all(&sandbox_root).map_err(|error| error.to_string())?;
        let context_project_dir = normalize_optional(request.context_project_dir)
            .or_else(|| normalize_optional(Some(request.project_dir.clone())))
            .map(|value| truncate(&value, MAX_TEXT_CHARS));
        let authorized_boundary = normalize_optional(request.access_project_dir.clone());
        let mut prepared_source = match prepare_source_repository(
            &request.project_dir,
            &sandbox_root,
            authorized_boundary.as_deref(),
        ) {
            Ok(source) => source,
            Err(error) => {
                let _ = fs::remove_dir_all(&sandbox_root);
                return Err(error);
            }
        };
        prepared_source.requested_project_dir = context_project_dir;
        let access_project_dir = authorized_boundary.or(prepared_source.access_project_dir);
        let project_dir = prepared_source.repository;
        let base_commit = prepared_source.base_commit;
        let test_command = if prepared_source.source_kind == OrchestrationSourceKind::GitClean {
            requested_test_command
        } else {
            "git diff --check (validation interne confinee)".to_string()
        };
        if let Err(error) = add_worktree(&project_dir, &orchestrator_dir, &base_commit) {
            let _ = fs::remove_dir_all(&sandbox_root);
            return Err(error);
        }

        let now = metrics::now_ts();
        let mut run = OrchestrationSnapshot {
            id,
            name,
            objective,
            worker_count,
            tester_count: required_tester_count(worker_count),
            adaptive_fanout,
            max_task_count,
            minimum_task_count,
            max_concurrency,
            account_id: orchestrator_account_id.clone(),
            orchestrator_account_id,
            worker_account_ids,
            orchestrator_handoff_pending: false,
            orchestrator_handoff_count: 0,
            owner_id: normalize_optional(request.owner_id),
            source_kind: prepared_source.source_kind,
            requested_project_dir: prepared_source.requested_project_dir,
            access_project_dir,
            project_dir: project_dir.to_string_lossy().to_string(),
            model: normalize_optional(request.model),
            reasoning_effort: normalize_optional(request.reasoning_effort),
            test_command,
            test_timeout_seconds,
            status: if start_paused {
                OrchestrationStatus::Paused
            } else {
                OrchestrationStatus::Active
            },
            phase: OrchestrationPhase::Planning,
            created_at: now,
            updated_at: now,
            base_commit: base_commit.clone(),
            integrated_commit: base_commit,
            sandbox_root: sandbox_root.to_string_lossy().to_string(),
            orchestrator_dir: orchestrator_dir.to_string_lossy().to_string(),
            orchestrator_session_id: orchestrator_session_id.clone(),
            current_turn_id: None,
            current_turn_kind: None,
            current_task_id: None,
            current_tester_id: None,
            current_start_id: None,
            current_validation_id: None,
            current_validation_kind: None,
            next_action_at: if start_paused { None } else { Some(now) },
            plan_summary: None,
            tasks: Vec::new(),
            testers: Vec::new(),
            merge_reviewed_task_ids: Vec::new(),
            final_summary: None,
            last_error: None,
            consecutive_start_failures: 0,
            protocol_failures: 0,
            publish_applied: false,
            team_messages: Vec::new(),
            events: Vec::new(),
        };
        push_event(
            &mut run,
            now,
            if orchestrator_session_id.is_some() {
                "promoted"
            } else {
                "created"
            },
            if orchestrator_session_id.is_some() {
                format!(
                    "Chat existant promu orchestrateur ; {worker_count} worker{} et {} orchestrateur{} testeur{} seront ouverts",
                    if worker_count > 1 { "s" } else { "" },
                    required_tester_count(worker_count),
                    if required_tester_count(worker_count) > 1 { "s" } else { "" },
                    if required_tester_count(worker_count) > 1 { "s" } else { "" }
                )
            } else {
                format!(
                    "Equipe creee : 1 orchestrateur principal, {worker_count} worker{} et {} orchestrateur{} testeur{}",
                    if worker_count > 1 { "s" } else { "" },
                    required_tester_count(worker_count),
                    if required_tester_count(worker_count) > 1 { "s" } else { "" },
                    if required_tester_count(worker_count) > 1 { "s" } else { "" }
                )
            },
        );
        match run.source_kind {
            OrchestrationSourceKind::GitClean => {}
            OrchestrationSourceKind::GitDirty => push_event(
                &mut run,
                now,
                "source_private_clone",
                "Depot source sale clone sans hardlink dans le sandbox ; aucune publication automatique".to_string(),
            ),
            OrchestrationSourceKind::Ephemeral => push_event(
                &mut run,
                now,
                "source_ephemeral",
                "Aucun depot Git utilisable : depot prive vide, sans copie ni modification du chemin demande".to_string(),
            ),
        }
        let created = run.clone();
        if let Err(error) = self.inner.mutate_store(|store| {
            store.runs.push(run);
            Ok(())
        }) {
            let _ = remove_owned_worktrees(&created, &self.inner.sandboxes_path);
            return Err(error);
        }
        Ok(created)
    }

    /// Transforme un agent autonome en orchestrateur sans jamais laisser les
    /// deux planificateurs piloter la même session. L'orchestration est d'abord
    /// créée en pause, puis l'agent autonome est retiré, et elle est seulement
    /// ensuite activée.
    pub fn promote_autonomous_agent(
        &self,
        autonomous: &AutonomousAgentManager,
        id: &str,
        request: PromoteAutonomousAgentRequest,
    ) -> Result<OrchestrationSnapshot, String> {
        let checkpoint = autonomous.prepare_orchestration_promotion(id)?;
        let agent = &checkpoint.snapshot;
        let create_request = CreateOrchestrationRequest {
            name: request.name,
            objective: request.objective,
            worker_count: request.worker_count,
            adaptive_fanout: request.adaptive_fanout,
            max_task_count: request.max_task_count,
            max_concurrency: request.max_concurrency,
            orchestrator_session_id: agent.session_id.clone(),
            orchestrator_account_id: Some(agent.account_id.clone()),
            worker_account_ids: request.worker_account_ids,
            account_id: agent.account_id.clone(),
            owner_id: request.owner_id,
            context_project_dir: None,
            access_project_dir: request.access_project_dir,
            project_dir: request.project_dir,
            model: agent.model.clone(),
            reasoning_effort: agent.reasoning_effort.clone(),
            test_command: request.test_command,
            test_timeout_seconds: request.test_timeout_seconds,
        };

        let created = match self.create_internal(create_request, true) {
            Ok(created) => created,
            Err(error) => {
                return match autonomous.rollback_orchestration_promotion(&checkpoint) {
                    Ok(_) => Err(error),
                    Err(rollback_error) => Err(format!(
                        "{error}. L'agent reste en pause car le retour arrière a échoué : {rollback_error}"
                    )),
                };
            }
        };

        if let Err(finalize_error) = autonomous.finalize_orchestration_promotion(id) {
            let orchestration_rollback = self.delete(&created.id);
            let autonomous_rollback = if orchestration_rollback.is_ok() {
                autonomous.rollback_orchestration_promotion(&checkpoint)
            } else {
                Err("orchestration non supprimée ; agent conservé en pause".to_string())
            };
            return match (orchestration_rollback, autonomous_rollback) {
                (Ok(()), Ok(_)) => Err(format!(
                    "Promotion annulée avant activation : {finalize_error}"
                )),
                (orchestration_result, autonomous_result) => Err(format!(
                    "Promotion interrompue ({finalize_error}). Nettoyage de l'orchestration : {}. Retour de l'agent : {}",
                    orchestration_result
                        .err()
                        .unwrap_or_else(|| "réussi".to_string()),
                    autonomous_result
                        .err()
                        .unwrap_or_else(|| "réussi".to_string())
                )),
            };
        }

        // L'agent autonome n'existe plus : à partir d'ici, même un échec de
        // persistance laisse une seule entité, l'orchestration en pause. Elle
        // reste visible et peut être reprise manuellement sans double exécution.
        match self.control(&created.id, OrchestrationAction::Resume) {
            Ok(active) => Ok(active),
            Err(error) => {
                eprintln!(
                    "[orchestration] promotion {} créée mais activation différée : {error}",
                    created.id
                );
                Ok(self
                    .list()?
                    .into_iter()
                    .find(|run| run.id == created.id)
                    .unwrap_or(created))
            }
        }
    }

    pub fn control(
        &self,
        id: &str,
        action: OrchestrationAction,
    ) -> Result<OrchestrationSnapshot, String> {
        let _lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| "Cycle de vie des orchestrations verrouille".to_string())?;
        let now = metrics::now_ts();
        let mut turn_to_stop = None;
        let mut validation_to_cancel = None;
        let snapshot = self.inner.mutate_store(|store| {
            let run = find_run_mut(store, id)?;
            match action {
                OrchestrationAction::Pause => {
                    if run.status == OrchestrationStatus::Completed {
                        return Err(
                            "Une orchestration terminee ne peut pas etre mise en pause".to_string()
                        );
                    }
                    turn_to_stop = run.current_turn_id.take();
                    validation_to_cancel = run.current_validation_id.take();
                    run.current_turn_kind = None;
                    run.current_validation_kind = None;
                    run.current_start_id = None;
                    run.status = OrchestrationStatus::Paused;
                    run.next_action_at = None;
                    push_event(
                        run,
                        now,
                        "paused",
                        "Orchestration mise en pause".to_string(),
                    );
                }
                OrchestrationAction::Resume | OrchestrationAction::Retry => {
                    if run.status == OrchestrationStatus::Completed {
                        return Err("Cette orchestration est deja terminee".to_string());
                    }
                    recover_phase_for_resume(run, now);
                    run.status = OrchestrationStatus::Active;
                    run.last_error = None;
                    run.consecutive_start_failures = 0;
                    run.protocol_failures = 0;
                    for task in &mut run.tasks {
                        if task.status != OrchestrationTaskStatus::Accepted {
                            task.protocol_failures = 0;
                        }
                    }
                    run.next_action_at = Some(now);
                    push_event(run, now, "resumed", "Orchestration reprise".to_string());
                }
            }
            run.updated_at = now;
            Ok(run.clone())
        })?;
        if let Some(validation_id) = validation_to_cancel {
            self.inner.cancel_validation(id, &validation_id);
        }
        if let Some(turn_id) = turn_to_stop {
            let _ = self.inner.chat.stop(turn_id);
        }
        Ok(snapshot)
    }

    pub fn reassign_account(
        &self,
        id: &str,
        request: ReassignOrchestrationAccountRequest,
    ) -> Result<OrchestrationSnapshot, String> {
        let _lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| "Cycle de vie des orchestrations verrouille".to_string())?;
        let target_account_id = request.account_id.trim().to_string();
        if target_account_id.is_empty() {
            return Err("Compte cible obligatoire".to_string());
        }
        let app_settings = settings::load_settings_for_terminal()?;
        let target_label = require_authenticated_account(&app_settings, &target_account_id)?
            .label
            .clone();
        let run = snapshot_run(&self.inner, id)?;
        let worker_index = match request.role {
            OrchestrationAccountRole::Orchestrator => None,
            OrchestrationAccountRole::Worker => {
                let index = request
                    .worker_index
                    .ok_or_else(|| "Numero du worker obligatoire".to_string())?;
                if index == 0 || index > run.worker_count {
                    return Err(format!(
                        "Worker invalide : choisis un numero entre 1 et {}",
                        run.worker_count
                    ));
                }
                Some(index)
            }
        };
        let task = worker_index.and_then(|index| {
            run.tasks
                .iter()
                .find(|task| task.position == index)
                .cloned()
        });
        let (source_account_id, stored_session_id, handoff_pending) = match request.role {
            OrchestrationAccountRole::Orchestrator => (
                resolved_orchestrator_account(&run).to_string(),
                run.orchestrator_session_id.clone(),
                run.orchestrator_handoff_pending,
            ),
            OrchestrationAccountRole::Worker => {
                let index = worker_index.unwrap_or(1);
                (
                    task.as_ref()
                        .map(|task| resolved_worker_account(&run, task).to_string())
                        .unwrap_or_else(|| {
                            run.worker_account_ids
                                .get((index - 1) as usize)
                                .filter(|value| !value.trim().is_empty())
                                .cloned()
                                .unwrap_or_else(|| run.account_id.clone())
                        }),
                    task.as_ref().and_then(|task| task.session_id.clone()),
                    task.as_ref().is_some_and(|task| task.handoff_pending),
                )
            }
        };
        if source_account_id == target_account_id {
            return Ok(run);
        }

        if request.role == OrchestrationAccountRole::Orchestrator
            && matches!(
                run.current_turn_kind,
                Some(OrchestrationTurnKind::TesterPlan | OrchestrationTurnKind::TesterValidation)
            )
        {
            return Err(
                "Un orchestrateur testeur utilise encore ce compte. Attends la fin de son tour."
                    .to_string(),
            );
        }
        let targets_current_turn = match (request.role, run.current_turn_kind) {
            (OrchestrationAccountRole::Orchestrator, Some(kind)) => matches!(
                kind,
                OrchestrationTurnKind::Plan
                    | OrchestrationTurnKind::Review
                    | OrchestrationTurnKind::MergeReview
                    | OrchestrationTurnKind::FinalReview
            ),
            (OrchestrationAccountRole::Worker, Some(OrchestrationTurnKind::Worker)) => task
                .as_ref()
                .is_some_and(|task| run.current_task_id.as_deref() == Some(task.id.as_str())),
            _ => false,
        };
        if targets_current_turn && run.current_start_id.is_some() {
            return Err(
                "Ce membre initialise son tour. Reessaie dans quelques secondes.".to_string(),
            );
        }

        let mut session_id = stored_session_id;
        if targets_current_turn {
            if let Some(turn_id) = run.current_turn_id {
                let now = metrics::now_ts();
                self.inner.mutate_store(|store| {
                    let current = find_run_mut(store, id)?;
                    if current.current_turn_id != Some(turn_id) {
                        return Err("Le tour a change pendant la reprise ; reessaie".to_string());
                    }
                    current.current_turn_id = None;
                    current.current_turn_kind = None;
                    current.next_action_at = None;
                    current.updated_at = now;
                    push_event(
                        current,
                        now,
                        "account_handoff_stopping",
                        "Tour actif arrete pour changer de compte sans execution concurrente"
                            .to_string(),
                    );
                    Ok(())
                })?;
                match self.inner.chat.stop(turn_id) {
                    Ok(snapshot) => {
                        session_id = snapshot.session_id.or(session_id);
                    }
                    Err(error) => {
                        mark_needs_attention(
                            &self.inner,
                            id,
                            format!("Reprise de compte impossible : {error}"),
                        );
                        return Err(error);
                    }
                }
            }
        }

        let prepared = match prepare_account_handoff(
            &run,
            request.role,
            worker_index,
            &source_account_id,
            &target_account_id,
            session_id,
            handoff_pending,
            &app_settings,
        ) {
            Ok(prepared) => prepared,
            Err(error) => {
                let now = metrics::now_ts();
                let _ = self.inner.mutate_store(|store| {
                    let current = find_run_mut(store, id)?;
                    if current.status == OrchestrationStatus::Active
                        && current.current_turn_id.is_none()
                        && current.current_start_id.is_none()
                        && current.current_validation_id.is_none()
                    {
                        current.next_action_at = Some(now);
                    }
                    current.updated_at = now;
                    push_event(
                        current,
                        now,
                        "account_handoff_failed",
                        format!("Changement de compte annule : {error}"),
                    );
                    Ok(())
                });
                return Err(error);
            }
        };

        let now = metrics::now_ts();
        let role_label = worker_index
            .map(|index| format!("Worker {index}"))
            .unwrap_or_else(|| "Orchestrateur".to_string());
        let updated = self.inner.mutate_store(|store| {
            let current = find_run_mut(store, id)?;
            match request.role {
                OrchestrationAccountRole::Orchestrator => {
                    if resolved_orchestrator_account(current) != source_account_id {
                        return Err(
                            "Le compte orchestrateur a deja change ; actualise la vue".to_string()
                        );
                    }
                    current.orchestrator_account_id = target_account_id.clone();
                    current.orchestrator_session_id = prepared.session_id.clone();
                    current.orchestrator_handoff_pending = prepared.handoff_pending;
                    current.orchestrator_handoff_count =
                        current.orchestrator_handoff_count.saturating_add(1);
                    for tester in &mut current.testers {
                        tester.session_id = None;
                    }
                }
                OrchestrationAccountRole::Worker => {
                    let index = worker_index.unwrap_or(1);
                    if current.worker_account_ids.len() < current.worker_count as usize {
                        current
                            .worker_account_ids
                            .resize(current.worker_count as usize, current.account_id.clone());
                    }
                    current.worker_account_ids[(index - 1) as usize] = target_account_id.clone();
                    if let Some(task) = current.tasks.iter_mut().find(|task| task.position == index)
                    {
                        task.account_id = target_account_id.clone();
                        task.session_id = prepared.session_id.clone();
                        task.handoff_pending = prepared.handoff_pending;
                        task.handoff_count = task.handoff_count.saturating_add(1);
                    }
                }
            }
            if current.status == OrchestrationStatus::Active
                && current.current_turn_id.is_none()
                && current.current_start_id.is_none()
                && current.current_validation_id.is_none()
            {
                current.next_action_at = Some(now);
            }
            current.updated_at = now;
            push_event(
                current,
                now,
                "account_reassigned",
                format!(
                    "{role_label} repris par {target_label} : {}",
                    prepared.summary
                ),
            );
            Ok(current.clone())
        });
        match updated {
            Ok(updated) => Ok(updated),
            Err(error) => {
                mark_needs_attention(&self.inner, id, error.clone());
                Err(error)
            }
        }
    }

    pub fn delete(&self, id: &str) -> Result<(), String> {
        let _lifecycle = self
            .inner
            .lifecycle
            .lock()
            .map_err(|_| "Cycle de vie des orchestrations verrouille".to_string())?;
        let now = metrics::now_ts();
        let (run, turn_to_stop, validation_to_cancel) = self.inner.mutate_store(|store| {
            let run = find_run_mut(store, id)?;
            let turn_to_stop = run.current_turn_id.take();
            let validation_to_cancel = run.current_validation_id.take();
            run.current_turn_kind = None;
            run.current_validation_kind = None;
            run.status = OrchestrationStatus::Paused;
            run.next_action_at = None;
            run.updated_at = now;
            push_event(
                run,
                now,
                "deleting",
                "Arret des travaux avant suppression des sandboxes".to_string(),
            );
            Ok((run.clone(), turn_to_stop, validation_to_cancel))
        })?;
        if let Some(validation_id) = validation_to_cancel.as_deref() {
            self.inner.cancel_validation(id, validation_id);
        }
        if let Some(turn_id) = turn_to_stop {
            self.inner
                .chat
                .stop(turn_id)
                .map_err(|error| format!("Arret du chat avant suppression impossible : {error}"))?;
        }
        self.inner.wait_until_quiescent(
            id,
            validation_to_cancel.as_deref(),
            DELETE_QUIESCE_TIMEOUT,
        )?;
        remove_owned_worktrees(&run, &self.inner.sandboxes_path)?;
        self.inner.mutate_store(|store| {
            let before = store.runs.len();
            store.runs.retain(|candidate| candidate.id != id);
            if store.runs.len() == before {
                return Err("Orchestration introuvable".to_string());
            }
            Ok(())
        })?;
        if let Ok(mut locks) = self.inner.run_locks.lock() {
            locks.remove(id);
        }
        Ok(())
    }
}

impl OrchestrationInner {
    fn run_lock(&self, run_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self
            .run_locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        locks.entry(run_id.to_string()).or_default().clone()
    }

    fn mutate_store<T>(
        &self,
        mutate: impl FnOnce(&mut OrchestrationStore) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut store = self
            .store
            .lock()
            .map_err(|_| "Etat des orchestrations verrouille".to_string())?;
        let previous = store.clone();
        let result = match mutate(&mut store) {
            Ok(result) => result,
            Err(error) => {
                *store = previous;
                return Err(error);
            }
        };
        if *store == previous {
            return Ok(result);
        }
        if let Err(error) = persist_store(&self.storage_path, &store) {
            *store = previous;
            return Err(error);
        }
        Ok(result)
    }

    fn work_items(&self, now: i64) -> Vec<WorkItem> {
        let Ok(store) = self.store.lock() else {
            return Vec::new();
        };
        // Le budget global est deja consomme : on ne programme aucun nouveau
        // demarrage, mais on continue a poller les tours en vol. La vraie
        // garantie est prise atomiquement dans `start_chat_turn` sous le
        // verrou du store ; ce raccourci evite seulement des tentatives vaines.
        let budget_reached = concurrency_budget_reached(&store);
        store
            .runs
            .iter()
            .filter(|run| run.status == OrchestrationStatus::Active)
            .filter_map(|run| {
                if let (Some(turn_id), Some(kind)) = (run.current_turn_id, run.current_turn_kind) {
                    return Some(WorkItem::Poll {
                        run_id: run.id.clone(),
                        turn_id,
                        kind,
                    });
                }
                if !budget_reached
                    && run.current_start_id.is_none()
                    && run.current_validation_id.is_none()
                    && run.next_action_at.is_some_and(|next| next <= now)
                {
                    return Some(WorkItem::Drive {
                        run_id: run.id.clone(),
                    });
                }
                None
            })
            .collect()
    }

    fn cancel_validation(&self, run_id: &str, validation_id: &str) {
        if let Ok(runs) = self.validation_runs.lock() {
            if let Some(run) = runs.get(run_id) {
                if run.id == validation_id {
                    run.cancelled.store(true, Ordering::SeqCst);
                }
            }
        }
    }

    fn wait_until_quiescent(
        &self,
        run_id: &str,
        validation_id: Option<&str>,
        timeout: Duration,
    ) -> Result<(), String> {
        let deadline = Instant::now() + timeout;
        loop {
            let start_active = self
                .store
                .lock()
                .map_err(|_| "Etat des orchestrations verrouille".to_string())?
                .runs
                .iter()
                .find(|run| run.id == run_id)
                .is_some_and(|run| run.current_start_id.is_some());
            let validation_active = if let Some(validation_id) = validation_id {
                self.validation_runs
                    .lock()
                    .map_err(|_| "Etat des validations verrouille".to_string())?
                    .get(run_id)
                    .is_some_and(|run| run.id == validation_id)
            } else {
                false
            };
            if !start_active && !validation_active {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(
                    "Suppression differee : un processus orchestre ne s'est pas encore arrete"
                        .to_string(),
                );
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

/// Programme plusieurs drivers asynchrones : chaque run est pilote sous son
/// propre verrou, donc les orchestrations et leurs agents progressent en
/// parallele au lieu d'etre serialises par une boucle unique. Le nombre de
/// drivers se regle via `CST_ORCHESTRATION_DRIVERS` (defaut 4, borne 1..=64).
fn spawn_drivers(inner: Weak<OrchestrationInner>) {
    let driver_count = configured_driver_count();
    for driver_index in 0..driver_count {
        let inner = inner.clone();
        let _ = thread::Builder::new()
            .name(format!("cst-orchestrated-chats-{driver_index}"))
            .spawn(move || driver_loop(inner));
    }
}

fn driver_loop(inner: Weak<OrchestrationInner>) {
    loop {
        let Some(inner) = inner.upgrade() else {
            break;
        };
        for item in inner.work_items(metrics::now_ts()) {
            let run_id = item.run_id().to_string();
            let lock = inner.run_lock(&run_id);
            let Ok(_guard) = lock.try_lock() else {
                // Un autre driver pilote deja ce run : on passe au suivant.
                continue;
            };
            match item {
                WorkItem::Drive { run_id } => drive_run(&inner, &run_id),
                WorkItem::Poll {
                    run_id,
                    turn_id,
                    kind,
                } => poll_turn(&inner, &run_id, turn_id, kind),
            }
        }
        drop(inner);
        thread::sleep(Duration::from_millis(250));
    }
}

fn configured_driver_count() -> usize {
    std::env::var(DRIVER_COUNT_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .map(|count| count.clamp(MIN_DRIVER_COUNT, MAX_DRIVER_COUNT))
        .unwrap_or(DEFAULT_DRIVER_COUNT)
}

/// Tours d'agents deja en vol (en cours d'execution ou en cours de demarrage)
/// a travers toutes les orchestrations actives.
fn in_flight_turns(store: &OrchestrationStore) -> usize {
    store
        .runs
        .iter()
        .filter(|run| run.status == OrchestrationStatus::Active)
        .filter(|run| run.current_turn_id.is_some() || run.current_start_id.is_some())
        .count()
}

fn concurrency_budget_reached_with(store: &OrchestrationStore, budget: usize) -> bool {
    in_flight_turns(store) >= budget
}

/// Budget global de tours d'agents en vol, regle par `CST_ORCHESTRATION_CONCURRENCY`.
fn concurrency_budget_reached(store: &OrchestrationStore) -> bool {
    concurrency_budget_reached_with(store, configured_default_max_concurrency() as usize)
}

fn drive_run(inner: &Arc<OrchestrationInner>, run_id: &str) {
    let run = match snapshot_run(inner, run_id) {
        Ok(run) => run,
        Err(error) => {
            eprintln!("[orchestration] execution de {run_id} impossible: {error}");
            return;
        }
    };
    match run.phase {
        OrchestrationPhase::Planning => start_plan_turn(inner, &run),
        OrchestrationPhase::DesigningTests => start_tester_plan_turn(inner, &run),
        OrchestrationPhase::Working => start_worker_turn(inner, &run),
        OrchestrationPhase::Reviewing => start_review_turn(inner, &run),
        OrchestrationPhase::Testing => start_tester_validation_turn(inner, &run),
        OrchestrationPhase::Merging => start_merge_review_turn(inner, &run),
        OrchestrationPhase::FinalReview => start_final_review_turn(inner, &run),
        OrchestrationPhase::Publishing => publish_run(inner, &run),
        OrchestrationPhase::Validating
        | OrchestrationPhase::FinalValidation
        | OrchestrationPhase::Completed => {}
    }
}

fn snapshot_run(inner: &OrchestrationInner, run_id: &str) -> Result<OrchestrationSnapshot, String> {
    inner
        .store
        .lock()
        .map_err(|_| "Etat des orchestrations verrouille".to_string())?
        .runs
        .iter()
        .find(|run| run.id == run_id)
        .cloned()
        .ok_or_else(|| "Orchestration introuvable".to_string())
}

fn resolved_orchestrator_account(run: &OrchestrationSnapshot) -> &str {
    if run.orchestrator_account_id.trim().is_empty() {
        &run.account_id
    } else {
        &run.orchestrator_account_id
    }
}

fn resolved_worker_account<'a>(
    run: &'a OrchestrationSnapshot,
    task: &'a OrchestrationTask,
) -> &'a str {
    if !task.account_id.trim().is_empty() {
        &task.account_id
    } else {
        run.worker_account_ids
            .get(task.position.saturating_sub(1) as usize)
            .filter(|value| !value.trim().is_empty())
            .map(String::as_str)
            .unwrap_or(&run.account_id)
    }
}

fn require_authenticated_account<'a>(
    app_settings: &'a settings::AppSettings,
    account_id: &str,
) -> Result<&'a settings::AccountProfile, String> {
    let account = app_settings
        .accounts
        .iter()
        .find(|candidate| candidate.id == account_id)
        .ok_or_else(|| format!("Compte introuvable : {account_id}"))?;
    if !orchestration_provider_supported(account.provider) {
        return Err(format!(
            "Compte incompatible avec l'orchestration structuree : {}. Freebuff est un TUI interactif ; choisis un compte Codex, Claude ou compatible API.",
            account.label
        ));
    }
    if !settings::account_has_auth_tokens(account) {
        return Err(format!(
            "Compte non authentifie : {}. Connecte ce compte avant de l'affecter a l'orchestration.",
            account.label
        ));
    }
    Ok(account)
}

fn orchestration_provider_supported(provider: settings::Provider) -> bool {
    provider != settings::Provider::Freebuff
}

fn normalize_worker_accounts(
    account_ids: Vec<String>,
    worker_count: u32,
    fallback_account_id: &str,
) -> Result<Vec<String>, String> {
    if !account_ids.is_empty() && account_ids.len() != worker_count as usize {
        return Err(format!(
            "Il faut affecter exactement {worker_count} compte{} aux workers",
            if worker_count > 1 { "s" } else { "" }
        ));
    }
    if account_ids
        .iter()
        .map(|value| value.trim())
        .any(|value| !value.is_empty() && value != fallback_account_id)
    {
        return Err(
            "Pendant la phase de test, tous les workers doivent utiliser le compte de l'orchestrateur"
                .to_string(),
        );
    }
    Ok(vec![fallback_account_id.to_string(); worker_count as usize])
}

fn handoff_file(
    run: &OrchestrationSnapshot,
    role: OrchestrationAccountRole,
    worker_index: Option<u32>,
) -> PathBuf {
    let name = match role {
        OrchestrationAccountRole::Orchestrator => "orchestrator.txt".to_string(),
        OrchestrationAccountRole::Worker => {
            format!("worker-{:02}.txt", worker_index.unwrap_or(1))
        }
    };
    Path::new(&run.sandbox_root).join("handoffs").join(name)
}

#[allow(clippy::too_many_arguments)]
fn prepare_account_handoff(
    run: &OrchestrationSnapshot,
    role: OrchestrationAccountRole,
    worker_index: Option<u32>,
    source_account_id: &str,
    target_account_id: &str,
    session_id: Option<String>,
    existing_handoff: bool,
    app_settings: &settings::AppSettings,
) -> Result<PreparedAccountHandoff, String> {
    let path = handoff_file(run, role, worker_index);
    let Some(session_id) = session_id else {
        return Ok(PreparedAccountHandoff {
            session_id: None,
            handoff_pending: existing_handoff,
            summary: if existing_handoff {
                "contexte de reprise conserve pour le prochain tour".to_string()
            } else {
                "affectation mise a jour avant le premier tour".to_string()
            },
        });
    };
    let source = app_settings
        .accounts
        .iter()
        .find(|account| account.id == source_account_id)
        .ok_or_else(|| format!("Compte source introuvable : {source_account_id}"))?;
    let target = app_settings
        .accounts
        .iter()
        .find(|account| account.id == target_account_id)
        .ok_or_else(|| format!("Compte cible introuvable : {target_account_id}"))?;
    if source.provider == settings::Provider::Codex && target.provider == settings::Provider::Codex
    {
        let copied = discussions::copy_discussion_between(
            session_id,
            source_account_id.to_string(),
            target_account_id.to_string(),
        )?;
        let _ = fs::remove_file(path);
        return Ok(PreparedAccountHandoff {
            session_id: Some(copied.rollout_id),
            handoff_pending: false,
            summary: "historique Codex copie et pret a reprendre".to_string(),
        });
    }

    let transcript =
        discussions::export_transcript_for_account(source_account_id.to_string(), session_id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs_util::atomic_write(&path, transcript).map_err(|error| error.to_string())?;
    Ok(PreparedAccountHandoff {
        session_id: None,
        handoff_pending: true,
        summary: "transcript securise ; une nouvelle session sera amorcee au prochain tour"
            .to_string(),
    })
}

/// Part maximale du relais consacree a l'historique. Laisse une marge
/// confortable sous `chat::MAX_PROMPT_BYTES` (256 Ko) pour l'instruction qui
/// lui est concatenee.
const HANDOFF_TRANSCRIPT_MAX_BYTES: usize = 96 * 1024;

fn prompt_with_pending_handoff(
    run: &OrchestrationSnapshot,
    kind: OrchestrationTurnKind,
    task_id: Option<&str>,
    prompt: String,
) -> Result<(String, Option<PathBuf>), String> {
    let (pending, role, worker_index) = if kind == OrchestrationTurnKind::Worker {
        let task_id = task_id.ok_or_else(|| "Worker absent pour la reprise".to_string())?;
        let task = run
            .tasks
            .iter()
            .find(|task| task.id == task_id)
            .ok_or_else(|| "Worker introuvable pour la reprise".to_string())?;
        (
            task.handoff_pending,
            OrchestrationAccountRole::Worker,
            Some(task.position),
        )
    } else if matches!(
        kind,
        OrchestrationTurnKind::Plan
            | OrchestrationTurnKind::Review
            | OrchestrationTurnKind::MergeReview
            | OrchestrationTurnKind::FinalReview
    ) {
        (
            run.orchestrator_handoff_pending,
            OrchestrationAccountRole::Orchestrator,
            None,
        )
    } else {
        return Ok((prompt, None));
    };
    if !pending {
        return Ok((prompt, None));
    }
    let path = handoff_file(run, role, worker_index);
    let transcript = fs::read_to_string(&path).map_err(|error| {
        format!(
            "Contexte de reprise introuvable ({}): {error}",
            path.display()
        )
    })?;
    // `transcript` et `prompt` etaient bornes chacun de leur cote, jamais leur
    // somme : le relais pouvait donc depasser `chat::MAX_PROMPT_BYTES` et faire
    // echouer la reprise. On borne la part historique, la seule compressible --
    // l'instruction de l'utilisateur, elle, part toujours en entier.
    let transcript = discussions::keep_last_bytes(&transcript, HANDOFF_TRANSCRIPT_MAX_BYTES);
    Ok((
        format!(
            "{transcript}\n\n[Nouvelle instruction du chat orchestre apres changement de compte]\n\n{prompt}"
        ),
        Some(path),
    ))
}

fn start_plan_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let prompt = plan_prompt(run);
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::Plan,
        None,
        None,
        resolved_orchestrator_account(run).to_string(),
        run.orchestrator_session_id.clone(),
        run.orchestrator_dir.clone(),
        ChatTurnMode::Plan,
        prompt,
    );
}

fn start_tester_plan_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let Some(tester) = run
        .testers
        .iter()
        .find(|tester| tester.status == OrchestrationTesterStatus::Pending)
        .cloned()
    else {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Working;
            current.current_tester_id = None;
            current.next_action_at = Some(now);
            current.updated_at = now;
            push_event(
                current,
                now,
                "test_plans_ready",
                format!(
                    "{} orchestrateur{} testeur{} ont defini les tests obligatoires",
                    current.testers.len(),
                    if current.testers.len() > 1 { "s" } else { "" },
                    if current.testers.len() > 1 { "s" } else { "" }
                ),
            );
            Ok(())
        });
        return;
    };
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::TesterPlan,
        None,
        Some(tester.id.clone()),
        resolved_orchestrator_account(run).to_string(),
        tester.session_id.clone(),
        run.orchestrator_dir.clone(),
        ChatTurnMode::Plan,
        tester_plan_prompt(run, &tester),
    );
}

fn start_tester_validation_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let Some(tester) = run
        .testers
        .iter()
        .find(|tester| tester.status != OrchestrationTesterStatus::Passed)
        .cloned()
    else {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            if !testers_all_passed(current) {
                return Err("Tous les orchestrateurs testeurs doivent valider le rendu".to_string());
            }
            let pending_merge = pending_merge_review_task_ids(current);
            current.phase = if pending_merge.is_empty() {
                OrchestrationPhase::FinalReview
            } else {
                OrchestrationPhase::Merging
            };
            current.current_tester_id = None;
            current.next_action_at = Some(now);
            current.updated_at = now;
            push_event(
                current,
                now,
                if pending_merge.is_empty() {
                    "tester_quorum_passed"
                } else {
                    "merge_review_scheduled"
                },
                if pending_merge.is_empty() {
                    "Tous les orchestrateurs testeurs ont valide leurs missions".to_string()
                } else {
                    format!(
                        "{} mission{} validee{} attend{} la fusion par le chat orchestrateur",
                        pending_merge.len(),
                        if pending_merge.len() > 1 { "s" } else { "" },
                        if pending_merge.len() > 1 { "s" } else { "" },
                        if pending_merge.len() > 1 { "ent" } else { "" }
                    )
                },
            );
            Ok(())
        });
        return;
    };
    if tester.test_plan.is_empty() {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            find_tester_mut(current, &tester.id)?.status = OrchestrationTesterStatus::Pending;
            current.phase = OrchestrationPhase::DesigningTests;
            current.next_action_at = Some(now);
            current.updated_at = now;
            Ok(())
        });
        return;
    }
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::TesterValidation,
        None,
        Some(tester.id.clone()),
        resolved_orchestrator_account(run).to_string(),
        tester.session_id.clone(),
        run.orchestrator_dir.clone(),
        ChatTurnMode::Build,
        tester_validation_prompt(run, &tester),
    );
}

fn start_merge_review_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let pending_task_ids = pending_merge_review_task_ids(run);
    if pending_task_ids.is_empty() {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = if testers_all_passed(current) {
                OrchestrationPhase::FinalReview
            } else {
                OrchestrationPhase::Testing
            };
            current.next_action_at = Some(now);
            current.updated_at = now;
            Ok(())
        });
        return;
    }
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::MergeReview,
        None,
        None,
        resolved_orchestrator_account(run).to_string(),
        run.orchestrator_session_id.clone(),
        run.orchestrator_dir.clone(),
        ChatTurnMode::Build,
        merge_review_prompt(run, &pending_task_ids),
    );
}

fn start_worker_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let Some(task) = next_open_task(run).cloned() else {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Testing;
            current.current_task_id = None;
            current.current_tester_id = None;
            current.next_action_at = Some(now);
            current.updated_at = now;
            push_event(
                current,
                now,
                "all_tasks_accepted",
                "Toutes les taches sont acceptees ; validation par les orchestrateurs testeurs"
                    .to_string(),
            );
            Ok(())
        });
        return;
    };
    let task = match ensure_worker_workspace(inner, run, &task) {
        Ok(task) => task,
        Err(error) => {
            mark_needs_attention(inner, &run.id, error);
            return;
        }
    };
    let prompt = worker_prompt(run, &task);
    let workspace = task.workspace_dir.clone().unwrap_or_default();
    let account_id = resolved_worker_account(run, &task).to_string();
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::Worker,
        Some(task.id.clone()),
        None,
        account_id,
        task.session_id.clone(),
        workspace,
        ChatTurnMode::Build,
        prompt,
    );
}

fn start_review_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    let Some(task_id) = run.current_task_id.as_deref() else {
        mark_needs_attention(inner, &run.id, "Tache de revue introuvable".to_string());
        return;
    };
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id).cloned() else {
        mark_needs_attention(inner, &run.id, "Tache de revue introuvable".to_string());
        return;
    };
    if task.status == OrchestrationTaskStatus::Submitted {
        if let Err(error) = apply_worker_candidate(run, &task) {
            request_revision(
                inner,
                &run.id,
                &task.id,
                format!("Le patch du travailleur ne s'applique pas proprement : {error}"),
            );
            return;
        }
        let now = metrics::now_ts();
        if let Err(error) = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            let current_task = find_task_mut(current, &task.id)?;
            current_task.status = OrchestrationTaskStatus::Reviewing;
            current.updated_at = now;
            Ok(())
        }) {
            mark_needs_attention(inner, &run.id, error);
            return;
        }
    }
    let refreshed = snapshot_run(inner, &run.id).unwrap_or_else(|_| run.clone());
    let Some(refreshed_task) = refreshed
        .tasks
        .iter()
        .find(|candidate| candidate.id == task.id)
    else {
        return;
    };
    let prompt = review_prompt(&refreshed, refreshed_task);
    start_chat_turn(
        inner,
        &refreshed,
        OrchestrationTurnKind::Review,
        Some(task.id),
        None,
        resolved_orchestrator_account(&refreshed).to_string(),
        refreshed.orchestrator_session_id.clone(),
        refreshed.orchestrator_dir.clone(),
        ChatTurnMode::Build,
        prompt,
    );
}

fn start_final_review_turn(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    if !pending_merge_review_task_ids(run).is_empty() {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Merging;
            current.next_action_at = Some(now);
            current.updated_at = now;
            Ok(())
        });
        return;
    }
    if !testers_all_passed(run) {
        let now = metrics::now_ts();
        let _ = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Testing;
            current.next_action_at = Some(now);
            current.updated_at = now;
            Ok(())
        });
        return;
    }
    if let Err(error) =
        reset_owned_worktree(Path::new(&run.orchestrator_dir), &run.integrated_commit)
    {
        mark_needs_attention(inner, &run.id, error);
        return;
    }
    start_chat_turn(
        inner,
        run,
        OrchestrationTurnKind::FinalReview,
        None,
        None,
        resolved_orchestrator_account(run).to_string(),
        run.orchestrator_session_id.clone(),
        run.orchestrator_dir.clone(),
        ChatTurnMode::Build,
        final_review_prompt(run),
    );
}

#[allow(clippy::too_many_arguments)]
fn start_chat_turn(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    kind: OrchestrationTurnKind,
    task_id: Option<String>,
    tester_id: Option<String>,
    account_id: String,
    session_id: Option<String>,
    project_dir: String,
    mode: ChatTurnMode,
    prompt: String,
) {
    if let Some(session_id) = session_id.as_deref() {
        match inner.chat.session_is_busy(&account_id, session_id) {
            // Un chat normal peut être promu pendant sa réponse courante. Le
            // run reste actif et prêt à démarrer ; le worker le réévaluera dès
            // que la session sera libre, sans annuler la réponse de l'utilisateur.
            Ok(true) => return,
            Ok(false) => {}
            Err(error) => {
                mark_needs_attention(inner, &run.id, error);
                return;
            }
        }
    }
    let (prompt, handoff_file_to_clear) =
        match prompt_with_pending_handoff(run, kind, task_id.as_deref(), prompt) {
            Ok(value) => value,
            Err(error) => {
                mark_needs_attention(inner, &run.id, error);
                return;
            }
        };
    let now = metrics::now_ts();
    let start_id = Uuid::new_v4().to_string();
    let prepared = inner.mutate_store(|store| {
        // Budget global de tours en vol : la reservation du tour et le comptage
        // se font sous le meme verrou du store, donc deux drivers ne peuvent
        // pas depasser `CST_ORCHESTRATION_CONCURRENCY` simultanement.
        if concurrency_budget_reached(store) {
            return Ok(false);
        }
        let current = find_run_mut(store, &run.id)?;
        if current.status != OrchestrationStatus::Active
            || current.current_turn_id.is_some()
            || current.current_start_id.is_some()
            || current.current_validation_id.is_some()
        {
            return Ok(false);
        }
        current.current_start_id = Some(start_id.clone());
        current.current_turn_kind = Some(kind);
        current.current_task_id = task_id.clone();
        current.current_tester_id = tester_id.clone();
        current.next_action_at = None;
        current.updated_at = now;
        if kind == OrchestrationTurnKind::Worker {
            if let Some(task_id) = task_id.as_deref() {
                let task = find_task_mut(current, task_id)?;
                task.status = OrchestrationTaskStatus::Working;
                task.attempt_count = task.attempt_count.saturating_add(1);
            }
        } else if matches!(
            kind,
            OrchestrationTurnKind::TesterPlan | OrchestrationTurnKind::TesterValidation
        ) {
            if let Some(tester_id) = tester_id.as_deref() {
                let tester = find_tester_mut(current, tester_id)?;
                tester.status = if kind == OrchestrationTurnKind::TesterPlan {
                    OrchestrationTesterStatus::Designing
                } else {
                    OrchestrationTesterStatus::Testing
                };
                tester.attempt_count = tester.attempt_count.saturating_add(1);
            }
        }
        Ok(true)
    });
    if !matches!(prepared, Ok(true)) {
        return;
    }

    let request = StartChatTurnRequest {
        account_id: account_id.clone(),
        session_id,
        prompt,
        image_attachments: Vec::new(),
        project_dir: Some(project_dir),
        mode,
        model: (account_id == run.account_id)
            .then(|| run.model.clone())
            .flatten(),
        reasoning_effort: (account_id == run.account_id)
            .then(|| run.reasoning_effort.clone())
            .flatten(),
        app_connectors: None,
        app_write_approved: false,
        agent_tools: Vec::new(),
        agent_skills: Vec::new(),
        question_tool: false,
        proof_tool: kind == OrchestrationTurnKind::Worker,
        source_chat_key: Some(match task_id.as_deref() {
            Some(task_id) => format!("orchestration:{}:{task_id}", run.id),
            None => tester_id
                .as_deref()
                .map(|tester_id| format!("orchestration:{}:{tester_id}", run.id))
                .unwrap_or_else(|| format!("orchestration:{}:orchestrator", run.id)),
        }),
    };
    match inner.chat.start_orchestration(request) {
        Ok(snapshot) => {
            if let Some(owner_id) = run.owner_id.as_deref() {
                if let Err(error) = inner.chat.assign_owner(snapshot.id, owner_id) {
                    let _ = inner.chat.stop(snapshot.id);
                    mark_needs_attention(
                        inner,
                        &run.id,
                        format!("Attribution du proprietaire du sous-chat impossible : {error}"),
                    );
                    return;
                }
            }
            let mut should_stop = false;
            let mut state_error = None;
            let mut handoff_consumed = false;
            let result = inner.mutate_store(|store| {
                let current = find_run_mut(store, &run.id)?;
                if current.current_start_id.as_deref() != Some(start_id.as_str()) {
                    should_stop = true;
                    return Ok(());
                }
                current.current_start_id = None;
                if current.status != OrchestrationStatus::Active {
                    should_stop = true;
                    return Ok(());
                }
                current.current_turn_id = Some(snapshot.id);
                current.consecutive_start_failures = 0;
                if let Some(found_session) = snapshot.session_id.clone() {
                    assign_session(
                        current,
                        kind,
                        task_id.as_deref(),
                        tester_id.as_deref(),
                        found_session,
                    )?;
                }
                if handoff_file_to_clear.is_some() && snapshot.session_id.is_some() {
                    if kind == OrchestrationTurnKind::Worker {
                        let task_id = task_id
                            .as_deref()
                            .ok_or_else(|| "Worker absent pour terminer la reprise".to_string())?;
                        find_task_mut(current, task_id)?.handoff_pending = false;
                    } else {
                        current.orchestrator_handoff_pending = false;
                    }
                    handoff_consumed = true;
                }
                current.updated_at = metrics::now_ts();
                Ok(())
            });
            let state_updated = result.is_ok();
            if let Err(error) = result {
                should_stop = true;
                state_error = Some(error);
            }
            if should_stop {
                let _ = inner.chat.stop(snapshot.id);
            }
            if let Some(error) = state_error {
                record_start_failure(inner, &run.id, &start_id, error);
            }
            if state_updated && handoff_consumed && !should_stop {
                if let Some(path) = handoff_file_to_clear {
                    let _ = fs::remove_file(path);
                }
            }
        }
        Err(error) => record_start_failure(inner, &run.id, &start_id, error),
    }
}

fn poll_turn(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    turn_id: u64,
    kind: OrchestrationTurnKind,
) {
    let snapshot = match inner.chat.status(turn_id) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            mark_needs_attention(inner, run_id, error);
            return;
        }
    };
    if snapshot.session_id.is_some() {
        let mut consumed_handoff = None;
        let updated = inner.mutate_store(|store| {
            let run = find_run_mut(store, run_id)?;
            if run.current_turn_id != Some(turn_id) {
                return Ok(());
            }
            let task_id = run.current_task_id.clone();
            let tester_id = run.current_tester_id.clone();
            assign_session(
                run,
                kind,
                task_id.as_deref(),
                tester_id.as_deref(),
                snapshot.session_id.clone().unwrap_or_default(),
            )?;
            let worker_index = if kind == OrchestrationTurnKind::Worker {
                let task_id = task_id
                    .as_deref()
                    .ok_or_else(|| "Worker absent pendant la reprise".to_string())?;
                let task = find_task_mut(run, task_id)?;
                if task.handoff_pending {
                    task.handoff_pending = false;
                    Some(task.position)
                } else {
                    None
                }
            } else if matches!(
                kind,
                OrchestrationTurnKind::Plan
                    | OrchestrationTurnKind::Review
                    | OrchestrationTurnKind::MergeReview
                    | OrchestrationTurnKind::FinalReview
            ) && run.orchestrator_handoff_pending
            {
                run.orchestrator_handoff_pending = false;
                Some(0)
            } else {
                None
            };
            if let Some(index) = worker_index {
                consumed_handoff = Some(handoff_file(
                    run,
                    if kind == OrchestrationTurnKind::Worker {
                        OrchestrationAccountRole::Worker
                    } else {
                        OrchestrationAccountRole::Orchestrator
                    },
                    (index > 0).then_some(index),
                ));
            }
            Ok(())
        });
        if updated.is_ok() {
            if let Some(path) = consumed_handoff {
                let _ = fs::remove_file(path);
            }
        }
    }
    match snapshot.status {
        ChatTurnStatus::Running | ChatTurnStatus::Finalizing => {}
        ChatTurnStatus::Completed => complete_turn(inner, run_id, turn_id, kind, &snapshot),
        ChatTurnStatus::Failed | ChatTurnStatus::Cancelled => {
            mark_needs_attention(
                inner,
                run_id,
                snapshot.error.unwrap_or_else(|| {
                    if snapshot.status == ChatTurnStatus::Cancelled {
                        "Le chat orchestre a ete annule".to_string()
                    } else {
                        "Le chat orchestre a echoue".to_string()
                    }
                }),
            );
        }
    }
}

fn complete_turn(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    turn_id: u64,
    kind: OrchestrationTurnKind,
    snapshot: &ChatTurnSnapshot,
) {
    let current = match snapshot_run(inner, run_id) {
        Ok(run) if run.current_turn_id == Some(turn_id) => run,
        _ => return,
    };
    match kind {
        OrchestrationTurnKind::Plan => complete_plan(inner, &current, snapshot),
        OrchestrationTurnKind::TesterPlan => complete_tester_plan(inner, &current, snapshot),
        OrchestrationTurnKind::Worker => complete_worker(inner, &current, snapshot),
        OrchestrationTurnKind::Review => complete_review(inner, &current, snapshot),
        OrchestrationTurnKind::TesterValidation => {
            complete_tester_validation(inner, &current, snapshot)
        }
        OrchestrationTurnKind::MergeReview => complete_merge_review(inner, &current, snapshot),
        OrchestrationTurnKind::FinalReview => complete_final_review(inner, &current, snapshot),
    }
}

fn complete_plan(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let text = snapshot_text(snapshot);
    let plan = match parse_marked_json::<PlanEnvelope>(&text, "ORCHESTRATION_PLAN:")
        .and_then(|plan| validate_plan(plan, run))
    {
        Ok(plan) => plan,
        Err(error) => {
            protocol_failure(inner, &run.id, None, format!("Plan invalide : {error}"));
            return;
        }
    };
    let now = metrics::now_ts();
    let result = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::Plan, snapshot)?;
        current.plan_summary = Some(plan.summary.clone());
        current.protocol_failures = 0;
        current.worker_count = plan.tasks.len() as u32;
        let mut worker_account_ids = current.worker_account_ids.clone();
        let fallback_account_id = current.account_id.clone();
        worker_account_ids.resize(current.worker_count as usize, fallback_account_id.clone());
        current.worker_account_ids = worker_account_ids.clone();
        current.tasks = plan
            .tasks
            .into_iter()
            .enumerate()
            .map(|(index, task)| OrchestrationTask {
                id: format!("task-{:02}", index + 1),
                position: (index + 1) as u32,
                title: task.title,
                description: task.description,
                acceptance_criteria: task.acceptance_criteria,
                status: OrchestrationTaskStatus::Pending,
                account_id: worker_account_ids
                    .get(index)
                    .cloned()
                    .unwrap_or_else(|| fallback_account_id.clone()),
                handoff_pending: false,
                handoff_count: 0,
                session_id: None,
                workspace_dir: None,
                base_commit: None,
                workspace_generation: 0,
                attempt_count: 0,
                protocol_failures: 0,
                evidence: None,
                reviews: Vec::new(),
                last_error: None,
            })
            .collect();
        current.tester_count = required_tester_count(current.worker_count);
        current.testers = build_testers(&current.tasks);
        current.phase = OrchestrationPhase::DesigningTests;
        current.current_tester_id = None;
        current.next_action_at = Some(now);
        current.last_error = None;
        current.updated_at = now;
        push_event(
            current,
            now,
            "plan_accepted",
            format!(
                "Plan accepte : {} chats travailleurs crees",
                current.tasks.len()
            ),
        );
        Ok(())
    });
    if let Err(error) = result {
        mark_needs_attention(inner, &run.id, error);
    }
}

fn complete_tester_plan(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let Some(tester_id) = run.current_tester_id.as_deref() else {
        mark_needs_attention(
            inner,
            &run.id,
            "Orchestrateur testeur absent pendant la conception des tests".to_string(),
        );
        return;
    };
    let text = snapshot_text(snapshot);
    let mut envelope =
        match parse_marked_json::<TesterPlanEnvelope>(&text, "ORCHESTRATION_TEST_PLAN:")
            .and_then(validate_tester_plan)
        {
            Ok(plan) => plan,
            Err(error) => {
                protocol_failure(
                    inner,
                    &run.id,
                    None,
                    format!("Plan de tests invalide : {error}"),
                );
                return;
            }
        };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            None,
            format!("Messages du testeur invalides : {error}"),
        );
        return;
    }
    let now = metrics::now_ts();
    let result = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::TesterPlan, snapshot)?;
        let tester = find_tester_mut(current, tester_id)?;
        tester.status = OrchestrationTesterStatus::Ready;
        tester.plan_summary = Some(envelope.summary);
        tester.test_plan = envelope.tests;
        tester.protocol_failures = 0;
        tester.last_error = None;
        append_team_messages(
            current,
            OrchestrationAccountRole::Orchestrator,
            None,
            envelope.messages,
            now,
        );
        current.phase = OrchestrationPhase::DesigningTests;
        current.current_tester_id = None;
        current.next_action_at = Some(now);
        current.last_error = None;
        current.updated_at = now;
        push_event(
            current,
            now,
            "tester_plan_ready",
            format!("Orchestrateur testeur {} : plan de tests pret", tester_id),
        );
        Ok(())
    });
    if let Err(error) = result {
        mark_needs_attention(inner, &run.id, error);
    }
}

fn complete_worker(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let Some(task_id) = run.current_task_id.as_deref() else {
        mark_needs_attention(inner, &run.id, "Tache travailleur absente".to_string());
        return;
    };
    let Some(task) = run.tasks.iter().find(|task| task.id == task_id) else {
        mark_needs_attention(inner, &run.id, "Tache travailleur absente".to_string());
        return;
    };
    let text = snapshot_text(snapshot);
    let mut envelope = match parse_marked_json::<ProofEnvelope>(&text, "ORCHESTRATION_PROOF:")
        .and_then(validate_proof)
    {
        Ok(proof) => proof,
        Err(error) => {
            protocol_failure(
                inner,
                &run.id,
                Some(task_id),
                format!("Preuve travailleur invalide : {error}"),
            );
            return;
        }
    };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            Some(task_id),
            format!("Messages du travailleur invalides : {error}"),
        );
        return;
    }
    let workspace = task.workspace_dir.as_deref().unwrap_or_default();
    let base_commit = task.base_commit.as_deref().unwrap_or_default();
    let files_changed = match stage_and_changed_files(Path::new(workspace), base_commit) {
        Ok(files) if !files.is_empty() => files,
        Ok(_) => {
            protocol_failure(
                inner,
                &run.id,
                Some(task_id),
                "La preuve annonce un travail termine mais aucun fichier n'a change".to_string(),
            );
            return;
        }
        Err(error) => {
            mark_needs_attention(inner, &run.id, error);
            return;
        }
    };
    let now = metrics::now_ts();
    let result = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::Worker, snapshot)?;
        let task_title = {
            let current_task = find_task_mut(current, task_id)?;
            current_task.status = OrchestrationTaskStatus::Submitted;
            current_task.protocol_failures = 0;
            current_task.last_error = None;
            current_task.evidence = Some(OrchestrationProof {
                summary: envelope.summary,
                files_changed,
                tests: envelope.tests,
                risks: envelope.risks,
                submitted_at: now,
            });
            current_task.title.clone()
        };
        let message_count = envelope.messages.len();
        append_team_messages(
            current,
            OrchestrationAccountRole::Worker,
            Some(task_id),
            envelope.messages,
            now,
        );
        current.phase = OrchestrationPhase::Reviewing;
        current.current_task_id = Some(task_id.to_string());
        current.next_action_at = Some(now);
        current.last_error = None;
        current.updated_at = now;
        push_event(
            current,
            now,
            "proof_submitted",
            format!("{task_title} a soumis une preuve"),
        );
        if message_count > 0 {
            push_event(
                current,
                now,
                "team_message",
                format!(
                    "Worker {} a publie {message_count} message{} dans le groupe",
                    task.position,
                    if message_count > 1 { "s" } else { "" }
                ),
            );
        }
        Ok(())
    });
    if let Err(error) = result {
        mark_needs_attention(inner, &run.id, error);
    }
}

fn complete_review(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let Some(task_id) = run.current_task_id.as_deref() else {
        mark_needs_attention(inner, &run.id, "Tache de revue absente".to_string());
        return;
    };
    let text = snapshot_text(snapshot);
    let mut envelope = match parse_marked_json::<ReviewEnvelope>(&text, "ORCHESTRATION_REVIEW:")
        .and_then(validate_review)
    {
        Ok(review) => review,
        Err(error) => {
            protocol_failure(
                inner,
                &run.id,
                Some(task_id),
                format!("Revue orchestrateur invalide : {error}"),
            );
            return;
        }
    };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            Some(task_id),
            format!("Messages de l'orchestrateur invalides : {error}"),
        );
        return;
    }
    let now = metrics::now_ts();
    let decision = envelope.decision;
    let feedback = envelope.feedback.clone();
    let result = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::Review, snapshot)?;
        {
            let current_task = find_task_mut(current, task_id)?;
            current_task.reviews.push(OrchestrationReview {
                decision,
                summary: envelope.summary,
                feedback: feedback.clone(),
                tests: envelope.tests,
                created_at: now,
            });
            if current_task.reviews.len() > MAX_REVIEWS {
                current_task
                    .reviews
                    .drain(0..current_task.reviews.len() - MAX_REVIEWS);
            }
        }
        append_team_messages(
            current,
            OrchestrationAccountRole::Orchestrator,
            None,
            envelope.messages,
            now,
        );
        current.updated_at = now;
        Ok(())
    });
    if let Err(error) = result {
        mark_needs_attention(inner, &run.id, error);
        return;
    }
    match decision {
        OrchestrationReviewDecision::Revise => request_revision(
            inner,
            &run.id,
            task_id,
            if feedback.trim().is_empty() {
                "L'orchestrateur demande une revision".to_string()
            } else {
                feedback
            },
        ),
        OrchestrationReviewDecision::Accept => {
            if let Err(error) = begin_validation(
                inner,
                &run.id,
                OrchestrationValidationKind::Task,
                Some(task_id.to_string()),
            ) {
                mark_needs_attention(inner, &run.id, error);
            }
        }
    }
}

fn complete_tester_validation(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let Some(tester_id) = run.current_tester_id.as_deref() else {
        mark_needs_attention(
            inner,
            &run.id,
            "Orchestrateur testeur absent pendant la validation".to_string(),
        );
        return;
    };
    let text = snapshot_text(snapshot);
    let mut envelope =
        match parse_marked_json::<TesterResultEnvelope>(&text, "ORCHESTRATION_TEST_RESULT:")
            .and_then(|result| validate_tester_result(result, run))
        {
            Ok(result) => result,
            Err(error) => {
                protocol_failure(
                    inner,
                    &run.id,
                    None,
                    format!("Resultat du testeur invalide : {error}"),
                );
                return;
            }
        };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            None,
            format!("Messages du testeur invalides : {error}"),
        );
        return;
    }
    let now = metrics::now_ts();
    let decision = envelope.decision;
    let feedback = envelope.feedback.clone();
    let task_ids = envelope.task_ids.clone();
    let result = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::TesterValidation, snapshot)?;
        let tester = find_tester_mut(current, tester_id)?;
        tester.status = if decision == OrchestrationTesterDecision::Pass {
            OrchestrationTesterStatus::Passed
        } else {
            OrchestrationTesterStatus::RevisionRequired
        };
        tester.last_results = envelope.tests;
        tester.protocol_failures = 0;
        tester.last_error = if decision == OrchestrationTesterDecision::Pass {
            None
        } else {
            Some(feedback.clone())
        };
        append_team_messages(
            current,
            OrchestrationAccountRole::Orchestrator,
            None,
            envelope.messages,
            now,
        );
        current.current_tester_id = None;
        if decision == OrchestrationTesterDecision::Pass
            && !pending_merge_review_task_ids(current).is_empty()
        {
            current.phase = OrchestrationPhase::Merging;
        }
        current.next_action_at = (decision == OrchestrationTesterDecision::Pass).then_some(now);
        current.last_error = if decision == OrchestrationTesterDecision::Pass {
            None
        } else {
            Some(feedback.clone())
        };
        current.updated_at = now;
        push_event(
            current,
            now,
            if decision == OrchestrationTesterDecision::Pass {
                "tester_passed"
            } else {
                "tester_revision_requested"
            },
            if decision == OrchestrationTesterDecision::Pass {
                format!("Orchestrateur testeur {tester_id} : tous les tests passent")
            } else {
                format!(
                    "Orchestrateur testeur {tester_id} : correction demandee a {} worker{}",
                    task_ids.len(),
                    if task_ids.len() > 1 { "s" } else { "" }
                )
            },
        );
        Ok(())
    });
    if let Err(error) = result {
        mark_needs_attention(inner, &run.id, error);
        return;
    }
    if decision == OrchestrationTesterDecision::Revise {
        reopen_tasks(inner, &run.id, &task_ids, feedback);
    }
}

fn complete_merge_review(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let pending_task_ids = pending_merge_review_task_ids(run);
    if pending_task_ids.is_empty() {
        let now = metrics::now_ts();
        let update = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            clear_current_turn(current, OrchestrationTurnKind::MergeReview, snapshot)?;
            current.phase = if testers_all_passed(current) {
                OrchestrationPhase::FinalReview
            } else {
                OrchestrationPhase::Testing
            };
            current.next_action_at = Some(now);
            current.updated_at = now;
            Ok(())
        });
        if let Err(error) = update {
            mark_needs_attention(inner, &run.id, error);
        }
        return;
    }

    let text = snapshot_text(snapshot);
    let mut envelope = match parse_marked_json::<MergeReviewEnvelope>(&text, "ORCHESTRATION_MERGE:")
        .and_then(|review| validate_merge_review(review, run, &pending_task_ids))
    {
        Ok(review) => review,
        Err(error) => {
            protocol_failure(
                inner,
                &run.id,
                None,
                format!("Revue de fusion invalide : {error}"),
            );
            return;
        }
    };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            None,
            format!("Messages de fusion invalides : {error}"),
        );
        return;
    }

    let now = metrics::now_ts();
    if envelope.decision == OrchestrationReviewDecision::Revise {
        let task_ids = envelope.task_ids.clone();
        let feedback = envelope.feedback.clone();
        let update = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            clear_current_turn(current, OrchestrationTurnKind::MergeReview, snapshot)?;
            append_team_messages(
                current,
                OrchestrationAccountRole::Orchestrator,
                None,
                envelope.messages,
                now,
            );
            current.updated_at = now;
            Ok(())
        });
        if let Err(error) = update {
            mark_needs_attention(inner, &run.id, error);
            return;
        }
        reopen_tasks(inner, &run.id, &task_ids, feedback);
        return;
    }

    let integrated_commit = match commit_owned_worktree(
        run,
        &format!(
            "orchestration: fusion de {} sous-chat{}",
            pending_task_ids.len(),
            if pending_task_ids.len() > 1 { "s" } else { "" }
        ),
    ) {
        Ok(commit) => commit,
        Err(error) => {
            mark_needs_attention(inner, &run.id, error);
            return;
        }
    };
    let integration_changed = integrated_commit != run.integrated_commit;
    let summary = envelope.summary.clone();
    let update = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::MergeReview, snapshot)?;
        append_team_messages(
            current,
            OrchestrationAccountRole::Orchestrator,
            None,
            envelope.messages,
            now,
        );
        apply_merge_review_acceptance(
            current,
            &pending_task_ids,
            integrated_commit.clone(),
            integration_changed,
            &summary,
            now,
        )
    });
    if let Err(error) = update {
        mark_needs_attention(inner, &run.id, error);
    }
}

fn complete_final_review(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    snapshot: &ChatTurnSnapshot,
) {
    let text = snapshot_text(snapshot);
    let mut envelope = match parse_marked_json::<FinalEnvelope>(&text, "ORCHESTRATION_FINAL:")
        .and_then(|value| validate_final(value, run))
    {
        Ok(value) => value,
        Err(error) => {
            protocol_failure(
                inner,
                &run.id,
                None,
                format!("Audit final invalide : {error}"),
            );
            return;
        }
    };
    if let Err(error) = validate_team_messages(&mut envelope.messages, run) {
        protocol_failure(
            inner,
            &run.id,
            None,
            format!("Messages finaux de l'orchestrateur invalides : {error}"),
        );
        return;
    }
    let now = metrics::now_ts();
    if let Err(error) = inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        clear_current_turn(current, OrchestrationTurnKind::FinalReview, snapshot)?;
        current.protocol_failures = 0;
        current.final_summary = Some(envelope.summary.clone());
        append_team_messages(
            current,
            OrchestrationAccountRole::Orchestrator,
            None,
            envelope.messages.clone(),
            now,
        );
        current.updated_at = now;
        Ok(())
    }) {
        mark_needs_attention(inner, &run.id, error);
        return;
    }
    match envelope.decision {
        FinalDecision::Complete => {
            if let Err(error) =
                begin_validation(inner, &run.id, OrchestrationValidationKind::Final, None)
            {
                mark_needs_attention(inner, &run.id, error);
            }
        }
        FinalDecision::Revise => {
            let task_id = envelope.task_id.unwrap_or_default();
            reopen_task(
                inner,
                &run.id,
                &task_id,
                if envelope.feedback.trim().is_empty() {
                    "L'audit final demande une correction supplementaire".to_string()
                } else {
                    envelope.feedback
                },
            );
        }
    }
}

fn begin_validation(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    kind: OrchestrationValidationKind,
    task_id: Option<String>,
) -> Result<(), String> {
    let _lifecycle = inner
        .lifecycle
        .lock()
        .map_err(|_| "Cycle de vie des orchestrations verrouille".to_string())?;
    let now = metrics::now_ts();
    let validation_id = Uuid::new_v4().to_string();
    let run = inner.mutate_store(|store| {
        let run = find_run_mut(store, run_id)?;
        if run.status != OrchestrationStatus::Active || run.current_validation_id.is_some() {
            return Err("Cette orchestration ne peut pas lancer de validation".to_string());
        }
        run.current_validation_id = Some(validation_id.clone());
        run.current_validation_kind = Some(kind);
        run.current_task_id = task_id.clone();
        run.phase = if kind == OrchestrationValidationKind::Task {
            OrchestrationPhase::Validating
        } else {
            OrchestrationPhase::FinalValidation
        };
        if let Some(task_id) = task_id.as_deref() {
            find_task_mut(run, task_id)?.status = OrchestrationTaskStatus::Validating;
        }
        run.next_action_at = None;
        run.updated_at = now;
        push_event(
            run,
            now,
            "validation_started",
            if kind == OrchestrationValidationKind::Task {
                "Validation mecanique de la contribution".to_string()
            } else {
                "Validation mecanique finale".to_string()
            },
        );
        Ok(run.clone())
    })?;

    let validation = Arc::new(ValidationRun {
        id: validation_id.clone(),
        cancelled: AtomicBool::new(false),
    });
    if let Ok(mut runs) = inner.validation_runs.lock() {
        if let Some(previous) = runs.insert(run.id.clone(), validation.clone()) {
            previous.cancelled.store(true, Ordering::SeqCst);
        }
    }
    let thread_inner = Arc::clone(inner);
    let thread_run = run.clone();
    let thread_validation = validation.clone();
    let thread_task_id = task_id.clone();
    let spawned = thread::Builder::new()
        .name(format!("cst-orchestration-test-{}", short_id(&run.id)))
        .spawn(move || {
            let result = run_validation_command(&thread_run, &thread_validation.cancelled);
            finish_validation(
                &thread_inner,
                &thread_run.id,
                &thread_validation.id,
                kind,
                thread_task_id.as_deref(),
                result,
            );
            if let Ok(mut runs) = thread_inner.validation_runs.lock() {
                if runs
                    .get(&thread_run.id)
                    .is_some_and(|known| known.id == thread_validation.id)
                {
                    runs.remove(&thread_run.id);
                }
            }
        });
    if let Err(error) = spawned {
        finish_validation(
            inner,
            &run.id,
            &validation_id,
            kind,
            task_id.as_deref(),
            ValidationResult {
                passed: false,
                exit_code: None,
                duration_ms: 0,
                output: format!("Impossible de demarrer la validation : {error}"),
            },
        );
        if let Ok(mut runs) = inner.validation_runs.lock() {
            if runs
                .get(&run.id)
                .is_some_and(|known| known.id == validation_id)
            {
                runs.remove(&run.id);
            }
        }
    }
    Ok(())
}

fn finish_validation(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    validation_id: &str,
    kind: OrchestrationValidationKind,
    task_id: Option<&str>,
    result: ValidationResult,
) {
    let run = match snapshot_run(inner, run_id) {
        Ok(run) if run.current_validation_id.as_deref() == Some(validation_id) => run,
        _ => return,
    };
    if !result.passed {
        let message = format!(
            "Validation echouee{} apres {} ms :\n{}",
            result
                .exit_code
                .map(|code| format!(" (code {code})"))
                .unwrap_or_default(),
            result.duration_ms,
            result.output
        );
        if kind == OrchestrationValidationKind::Task {
            if let Some(task_id) = task_id {
                request_revision(inner, run_id, task_id, message);
            }
        } else {
            let now = metrics::now_ts();
            let _ = reset_owned_worktree(Path::new(&run.orchestrator_dir), &run.integrated_commit);
            let update = inner.mutate_store(|store| {
                let current = find_run_mut(store, run_id)?;
                if current.current_validation_id.as_deref() != Some(validation_id) {
                    return Ok(());
                }
                current.current_validation_id = None;
                current.current_validation_kind = None;
                reset_testers_for_validation(current, Some(&message));
                current.phase = OrchestrationPhase::Testing;
                current.current_tester_id = None;
                current.next_action_at = Some(now);
                current.last_error = Some(truncate(&message, MAX_TEXT_CHARS));
                current.updated_at = now;
                push_event(
                    current,
                    now,
                    "final_validation_failed",
                    "La commande finale a echoue ; les orchestrateurs testeurs doivent attribuer les corrections aux workers".to_string(),
                );
                Ok(())
            });
            if let Err(error) = update {
                mark_needs_attention(inner, run_id, error);
            }
        }
        return;
    }

    let commit_message = if kind == OrchestrationValidationKind::Task {
        let title = task_id
            .and_then(|id| run.tasks.iter().find(|task| task.id == id))
            .map(|task| task.title.as_str())
            .unwrap_or("contribution");
        format!("orchestration: {title}")
    } else {
        "orchestration: audit final".to_string()
    };
    let integrated_commit = match commit_owned_worktree(&run, &commit_message) {
        Ok(commit) => commit,
        Err(error) => {
            mark_needs_attention(inner, run_id, error);
            return;
        }
    };
    let now = metrics::now_ts();
    let update = inner.mutate_store(|store| {
        let current = find_run_mut(store, run_id)?;
        if current.current_validation_id.as_deref() != Some(validation_id) {
            return Ok(());
        }
        current.current_validation_id = None;
        current.current_validation_kind = None;
        current.integrated_commit = integrated_commit.clone();
        current.last_error = None;
        current.updated_at = now;
        if kind == OrchestrationValidationKind::Task {
            let task_id = task_id.ok_or_else(|| "Tache validee absente".to_string())?;
            let task_title = {
                let task = find_task_mut(current, task_id)?;
                task.status = OrchestrationTaskStatus::Accepted;
                task.last_error = None;
                task.title.clone()
            };
            current.phase = OrchestrationPhase::Working;
            current.current_task_id = None;
            current.next_action_at = Some(now);
            push_event(
                current,
                now,
                "task_accepted",
                format!("{task_title} acceptee et integree"),
            );
        } else {
            current.phase = OrchestrationPhase::Publishing;
            current.current_task_id = None;
            current.next_action_at = Some(now);
            push_event(
                current,
                now,
                "final_validation_passed",
                "Audit final et commande de validation reussis".to_string(),
            );
        }
        Ok(())
    });
    if let Err(error) = update {
        mark_needs_attention(inner, run_id, error);
    }
}

fn request_revision(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    task_id: &str,
    feedback: String,
) {
    let run = match snapshot_run(inner, run_id) {
        Ok(run) => run,
        Err(_) => return,
    };
    if let Err(error) =
        reset_owned_worktree(Path::new(&run.orchestrator_dir), &run.integrated_commit)
    {
        mark_needs_attention(inner, run_id, error);
        return;
    }
    let now = metrics::now_ts();
    let feedback = truncate(&feedback, MAX_TEXT_CHARS);
    let update = inner.mutate_store(|store| {
        let current = find_run_mut(store, run_id)?;
        current.current_turn_id = None;
        current.current_turn_kind = None;
        current.current_start_id = None;
        current.current_validation_id = None;
        current.current_validation_kind = None;
        let task_title = {
            let task = find_task_mut(current, task_id)?;
            task.status = OrchestrationTaskStatus::RevisionRequested;
            task.last_error = Some(feedback.clone());
            task.title.clone()
        };
        current.phase = OrchestrationPhase::Working;
        current.current_task_id = Some(task_id.to_string());
        current.next_action_at = Some(now);
        current.last_error = Some(feedback.clone());
        current.updated_at = now;
        push_event(
            current,
            now,
            "revision_requested",
            format!("Revision renvoyee a {task_title}"),
        );
        Ok(())
    });
    if let Err(error) = update {
        mark_needs_attention(inner, run_id, error);
    }
}

fn reopen_task(inner: &Arc<OrchestrationInner>, run_id: &str, task_id: &str, feedback: String) {
    reopen_tasks(inner, run_id, &[task_id.to_string()], feedback);
}

fn reopen_tasks(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    task_ids: &[String],
    feedback: String,
) {
    let run = match snapshot_run(inner, run_id) {
        Ok(run) => run,
        Err(_) => return,
    };
    if let Err(error) =
        reset_owned_worktree(Path::new(&run.orchestrator_dir), &run.integrated_commit)
    {
        mark_needs_attention(inner, run_id, error);
        return;
    }
    let now = metrics::now_ts();
    let feedback = truncate(&feedback, MAX_TEXT_CHARS);
    let update = inner.mutate_store(|store| {
        let current = find_run_mut(store, run_id)?;
        let task_titles = apply_tester_revision_state(current, task_ids, &feedback, now)?;
        push_event(
            current,
            now,
            "task_reopened",
            format!(
                "{} mission{} rouverte{} apres les tests : {}",
                task_titles.len(),
                if task_titles.len() > 1 { "s" } else { "" },
                if task_titles.len() > 1 { "s" } else { "" },
                task_titles.join(", ")
            ),
        );
        Ok(())
    });
    if let Err(error) = update {
        mark_needs_attention(inner, run_id, error);
    }
}

fn apply_tester_revision_state(
    run: &mut OrchestrationSnapshot,
    task_ids: &[String],
    feedback: &str,
    now: i64,
) -> Result<Vec<String>, String> {
    let mut task_titles = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let task = find_task_mut(run, task_id)?;
        task.status = OrchestrationTaskStatus::RevisionRequested;
        task.workspace_dir = None;
        task.base_commit = None;
        task.evidence = None;
        task.last_error = Some(feedback.to_string());
        task_titles.push(task.title.clone());
    }
    let reopened = task_ids.iter().map(String::as_str).collect::<HashSet<_>>();
    run.merge_reviewed_task_ids
        .retain(|task_id| !reopened.contains(task_id.as_str()));
    reset_testers_for_validation(run, Some(feedback));
    run.phase = OrchestrationPhase::Working;
    run.current_task_id = None;
    run.current_tester_id = None;
    run.next_action_at = Some(now);
    run.last_error = Some(feedback.to_string());
    run.updated_at = now;
    Ok(task_titles)
}

fn protocol_failure(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    task_id: Option<&str>,
    error: String,
) {
    let now = metrics::now_ts();
    let error = truncate(&error, MAX_TEXT_CHARS);
    let update = inner.mutate_store(|store| {
        let run = find_run_mut(store, run_id)?;
        if run.current_turn_kind == Some(OrchestrationTurnKind::TesterPlan) {
            if let Some(tester_id) = run.current_tester_id.clone() {
                find_tester_mut(run, &tester_id)?.status = OrchestrationTesterStatus::Pending;
            }
        } else if run.current_turn_kind == Some(OrchestrationTurnKind::TesterValidation) {
            if let Some(tester_id) = run.current_tester_id.clone() {
                find_tester_mut(run, &tester_id)?.status = OrchestrationTesterStatus::Ready;
            }
        }
        run.current_turn_id = None;
        run.current_turn_kind = None;
        run.current_start_id = None;
        let failures = if let Some(task_id) = task_id {
            let task = find_task_mut(run, task_id)?;
            task.protocol_failures = task.protocol_failures.saturating_add(1);
            task.last_error = Some(error.clone());
            task.protocol_failures
        } else {
            run.protocol_failures = run.protocol_failures.saturating_add(1);
            run.protocol_failures
        };
        run.last_error = Some(error.clone());
        run.updated_at = now;
        if failures >= MAX_PROTOCOL_FAILURES {
            run.status = OrchestrationStatus::NeedsAttention;
            run.next_action_at = None;
            push_event(
                run,
                now,
                "protocol_failed",
                "Protocole structure invalide trois fois ; intervention requise".to_string(),
            );
        } else {
            run.next_action_at = Some(now);
            push_event(
                run,
                now,
                "protocol_retry",
                format!(
                    "Nouvelle demande de format structure ({failures}/{MAX_PROTOCOL_FAILURES})"
                ),
            );
        }
        Ok(())
    });
    if let Err(persist_error) = update {
        eprintln!("[orchestration] erreur de protocole non persistee: {persist_error}");
    }
}

fn record_start_failure(
    inner: &Arc<OrchestrationInner>,
    run_id: &str,
    start_id: &str,
    error: String,
) {
    let now = metrics::now_ts();
    let transient = error.to_ascii_lowercase().contains("deja")
        || error.to_ascii_lowercase().contains("déjà")
        || error.to_ascii_lowercase().contains("already");
    let _ = inner.mutate_store(|store| {
        let run = find_run_mut(store, run_id)?;
        if run.current_start_id.as_deref() != Some(start_id) {
            return Ok(());
        }
        if run.current_turn_kind == Some(OrchestrationTurnKind::TesterPlan) {
            if let Some(tester_id) = run.current_tester_id.clone() {
                find_tester_mut(run, &tester_id)?.status = OrchestrationTesterStatus::Pending;
            }
        } else if run.current_turn_kind == Some(OrchestrationTurnKind::TesterValidation) {
            if let Some(tester_id) = run.current_tester_id.clone() {
                find_tester_mut(run, &tester_id)?.status = OrchestrationTesterStatus::Ready;
            }
        }
        run.current_start_id = None;
        run.current_turn_id = None;
        run.current_turn_kind = None;
        run.last_error = Some(truncate(&error, MAX_TEXT_CHARS));
        if transient {
            run.next_action_at = Some(now + 2);
        } else {
            run.consecutive_start_failures = run.consecutive_start_failures.saturating_add(1);
            if run.consecutive_start_failures >= MAX_START_FAILURES {
                run.status = OrchestrationStatus::NeedsAttention;
                run.next_action_at = None;
            } else {
                run.next_action_at = Some(now + 5);
            }
        }
        run.updated_at = now;
        Ok(())
    });
}

fn mark_needs_attention(inner: &Arc<OrchestrationInner>, run_id: &str, error: String) {
    let now = metrics::now_ts();
    let error = truncate(&error, MAX_TEXT_CHARS);
    if let Err(persist_error) = inner.mutate_store(|store| {
        let run = find_run_mut(store, run_id)?;
        run.status = OrchestrationStatus::NeedsAttention;
        run.current_turn_id = None;
        run.current_turn_kind = None;
        run.current_start_id = None;
        run.current_validation_id = None;
        run.current_validation_kind = None;
        run.next_action_at = None;
        run.last_error = Some(error.clone());
        run.updated_at = now;
        push_event(run, now, "needs_attention", error.clone());
        Ok(())
    }) {
        eprintln!("[orchestration] erreur non persistee pour {run_id}: {persist_error}");
    }
}

fn ensure_worker_workspace(
    inner: &Arc<OrchestrationInner>,
    run: &OrchestrationSnapshot,
    task: &OrchestrationTask,
) -> Result<OrchestrationTask, String> {
    if task
        .workspace_dir
        .as_deref()
        .is_some_and(|path| Path::new(path).is_dir())
        && task.base_commit.is_some()
    {
        return Ok(task.clone());
    }
    let generation = task.workspace_generation.saturating_add(1);
    let workspace = Path::new(&run.sandbox_root)
        .join("workers")
        .join(format!("{}-{:02}", task.id, generation));
    if let Some(parent) = workspace.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    add_worktree(
        Path::new(&run.project_dir),
        &workspace,
        &run.integrated_commit,
    )?;
    let workspace_text = workspace.to_string_lossy().to_string();
    inner.mutate_store(|store| {
        let current = find_run_mut(store, &run.id)?;
        let current_task = find_task_mut(current, &task.id)?;
        current_task.workspace_dir = Some(workspace_text.clone());
        current_task.base_commit = Some(run.integrated_commit.clone());
        current_task.workspace_generation = generation;
        Ok(current_task.clone())
    })
}

fn apply_worker_candidate(
    run: &OrchestrationSnapshot,
    task: &OrchestrationTask,
) -> Result<(), String> {
    let workspace = task
        .workspace_dir
        .as_deref()
        .ok_or_else(|| "Environnement du travailleur absent".to_string())?;
    let base = task
        .base_commit
        .as_deref()
        .ok_or_else(|| "Commit de base du travailleur absent".to_string())?;
    reset_owned_worktree(Path::new(&run.orchestrator_dir), &run.integrated_commit)?;
    stage_and_changed_files(Path::new(workspace), base)?;
    let patch_path = Path::new(&run.sandbox_root).join(format!("{}.patch", task.id));
    git_output_file(
        Path::new(workspace),
        ["diff", "--cached", "--binary", base],
        &patch_path,
    )?;
    let metadata = fs::metadata(&patch_path).map_err(|error| error.to_string())?;
    if metadata.len() == 0 {
        return Err("Le patch du travailleur est vide".to_string());
    }
    run_git(
        Path::new(&run.orchestrator_dir),
        ["apply", "--index", "--whitespace=nowarn"],
        Some(&patch_path),
    )?;
    Ok(())
}

fn publish_run(inner: &Arc<OrchestrationInner>, run: &OrchestrationSnapshot) {
    if !pending_merge_review_task_ids(run).is_empty() {
        let now = metrics::now_ts();
        let update = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Merging;
            current.next_action_at = Some(now);
            current.last_error = Some(
                "Publication bloquee : le chat orchestrateur doit fusionner les missions validees"
                    .to_string(),
            );
            current.updated_at = now;
            Ok(())
        });
        if let Err(error) = update {
            mark_needs_attention(inner, &run.id, error);
        }
        return;
    }
    if !testers_all_passed(run) {
        let now = metrics::now_ts();
        let update = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.phase = OrchestrationPhase::Testing;
            current.current_tester_id = None;
            current.next_action_at = Some(now);
            current.last_error = Some(
                "Publication bloquee : tous les orchestrateurs testeurs doivent valider"
                    .to_string(),
            );
            current.updated_at = now;
            Ok(())
        });
        if let Err(error) = update {
            mark_needs_attention(inner, &run.id, error);
        }
        return;
    }
    if run.source_kind != OrchestrationSourceKind::GitClean {
        let now = metrics::now_ts();
        let update = inner.mutate_store(|store| {
            let current = find_run_mut(store, &run.id)?;
            current.status = OrchestrationStatus::Completed;
            current.phase = OrchestrationPhase::Completed;
            current.publish_applied = false;
            current.next_action_at = None;
            current.last_error = None;
            current.updated_at = now;
            push_event(
                current,
                now,
                "completed_private",
                "Rendu valide conserve dans le sandbox prive ; aucune publication automatique sur la source"
                    .to_string(),
            );
            Ok(())
        });
        if let Err(error) = update {
            mark_needs_attention(inner, &run.id, error);
        }
        return;
    }
    match apply_final_patch(run) {
        Ok(()) => {
            let now = metrics::now_ts();
            let update = inner.mutate_store(|store| {
                let current = find_run_mut(store, &run.id)?;
                current.status = OrchestrationStatus::Completed;
                current.phase = OrchestrationPhase::Completed;
                current.publish_applied = true;
                current.next_action_at = None;
                current.last_error = None;
                current.updated_at = now;
                push_event(
                    current,
                    now,
                    "completed",
                    "Projet final valide et applique au dossier source".to_string(),
                );
                Ok(())
            });
            if let Err(error) = update {
                mark_needs_attention(inner, &run.id, error);
            }
        }
        Err(error) => mark_needs_attention(inner, &run.id, error),
    }
}

fn apply_final_patch(run: &OrchestrationSnapshot) -> Result<(), String> {
    if run.source_kind != OrchestrationSourceKind::GitClean {
        return Err("Publication interdite pour une source sale, non-Git ou ephemere".to_string());
    }
    let source = Path::new(&run.project_dir);
    let head = git_text(source, ["rev-parse", "HEAD"])?;
    if head.trim() != run.base_commit {
        return Err(
            "Le projet source a change de commit pendant l'orchestration. Le rendu reste disponible dans le sandbox orchestrateur et n'a pas ete applique."
                .to_string(),
        );
    }
    let patch_path = Path::new(&run.sandbox_root).join("final.patch");
    git_output_file(
        Path::new(&run.orchestrator_dir),
        ["diff", "--binary", &run.base_commit, &run.integrated_commit],
        &patch_path,
    )?;
    if fs::metadata(&patch_path)
        .map_err(|error| error.to_string())?
        .len()
        == 0
    {
        ensure_clean_repository(source)?;
        return Ok(());
    }
    if let Err(clean_error) = ensure_clean_repository(source) {
        if worktree_matches_commit(source, &run.integrated_commit, Path::new(&run.sandbox_root))? {
            return Ok(());
        }
        return Err(clean_error);
    }
    run_git(source, ["apply", "--check"], Some(&patch_path))?;
    run_git(source, ["apply", "--whitespace=nowarn"], Some(&patch_path))
}

fn worktree_matches_commit(path: &Path, commit: &str, scratch_dir: &Path) -> Result<bool, String> {
    let index_path = scratch_dir.join(format!(".publish-verify-{}.index", Uuid::new_v4()));
    let lock_path = index_path.with_extension("index.lock");
    let result = (|| {
        let read_tree = git_command_at(path)
            .env("GIT_INDEX_FILE", &index_path)
            .args(["read-tree", commit])
            .output()
            .map_err(|error| format!("Git est indisponible : {error}"))?;
        if !read_tree.status.success() {
            return Err(command_error(
                "Preparation de la verification du rendu impossible",
                &read_tree,
            ));
        }

        // `read-tree` connait les blobs attendus mais pas encore les metadonnees
        // du worktree courant. Le rafraichissement evite que Git signale chaque
        // fichier comme modifie uniquement parce que son stat cache est vide.
        let refresh = git_command_at(path)
            .env("GIT_INDEX_FILE", &index_path)
            .args(["update-index", "--refresh"])
            .output()
            .map_err(|error| format!("Git est indisponible : {error}"))?;
        if !refresh.status.success() && refresh.status.code() != Some(1) {
            return Err(command_error(
                "Rafraichissement de la verification du rendu impossible",
                &refresh,
            ));
        }

        let tracked = git_command_at(path)
            .env("GIT_INDEX_FILE", &index_path)
            .args(["diff-files", "--quiet", "--ignore-submodules"])
            .output()
            .map_err(|error| format!("Git est indisponible : {error}"))?;
        if !tracked.status.success() {
            if tracked.status.code() == Some(1) {
                return Ok(false);
            }
            return Err(command_error(
                "Comparaison du rendu applique impossible",
                &tracked,
            ));
        }

        let untracked = git_command_at(path)
            .env("GIT_INDEX_FILE", &index_path)
            .args(["ls-files", "--others", "--exclude-standard", "-z"])
            .output()
            .map_err(|error| format!("Git est indisponible : {error}"))?;
        if !untracked.status.success() {
            return Err(command_error(
                "Verification des fichiers supplementaires impossible",
                &untracked,
            ));
        }
        Ok(untracked.stdout.is_empty())
    })();
    let _ = fs::remove_file(&index_path);
    let _ = fs::remove_file(lock_path);
    result
}

fn commit_owned_worktree(run: &OrchestrationSnapshot, message: &str) -> Result<String, String> {
    let dir = Path::new(&run.orchestrator_dir);
    run_git(dir, ["add", "-A"], None)?;
    let status = git_status(dir, ["diff", "--cached", "--quiet", "--exit-code"])?;
    if status.success() {
        return git_text(dir, ["rev-parse", "HEAD"]);
    }
    run_git(
        dir,
        [
            "-c",
            "user.name=Codex Switch Orchestrator",
            "-c",
            "user.email=orchestrator@codex-switch.local",
            "-c",
            "commit.gpgSign=false",
            "-c",
            &format!("core.hooksPath={}", disabled_git_hooks_path()),
            "commit",
            "--no-gpg-sign",
            "-m",
            message,
        ],
        None,
    )?;
    git_text(dir, ["rev-parse", "HEAD"])
}

fn run_validation_command(run: &OrchestrationSnapshot, cancelled: &AtomicBool) -> ValidationResult {
    let started = Instant::now();
    if run.source_kind != OrchestrationSourceKind::GitClean {
        if cancelled.load(Ordering::SeqCst) {
            return ValidationResult {
                passed: false,
                exit_code: None,
                duration_ms: 0,
                output: "Validation annulee".to_string(),
            };
        }
        let output = git_command_at(Path::new(&run.orchestrator_dir))
            .args(["diff", "--check", &run.base_commit, "--"])
            .output();
        return match output {
            Ok(output) => ValidationResult {
                passed: output.status.success(),
                exit_code: output.status.code(),
                duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                output: truncate(
                    &String::from_utf8_lossy(if output.stdout.is_empty() {
                        &output.stderr
                    } else {
                        &output.stdout
                    }),
                    MAX_TEST_OUTPUT_BYTES,
                ),
            },
            Err(error) => ValidationResult {
                passed: false,
                exit_code: None,
                duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
                output: format!("Validation Git interne impossible : {error}"),
            },
        };
    }
    let mut command = shell_command(&run.test_command);
    command
        .current_dir(&run.orchestrator_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    hide_process_window(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return ValidationResult {
                passed: false,
                exit_code: None,
                duration_ms: 0,
                output: format!("Impossible de lancer la commande : {error}"),
            };
        }
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_thread = stdout.map(|stream| thread::spawn(move || read_capped(stream)));
    let stderr_thread = stderr.map(|stream| thread::spawn(move || read_capped(stream)));
    let timeout = Duration::from_secs(run.test_timeout_seconds);
    let mut exit = None;
    let mut forced_error = None;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            terminate_process_tree(&mut child);
            forced_error = Some("Validation annulee".to_string());
            break;
        }
        if started.elapsed() >= timeout {
            terminate_process_tree(&mut child);
            forced_error = Some(format!(
                "Validation interrompue apres le timeout de {} s",
                run.test_timeout_seconds
            ));
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                exit = Some(status);
                break;
            }
            Ok(None) => thread::sleep(Duration::from_millis(100)),
            Err(error) => {
                terminate_process_tree(&mut child);
                forced_error = Some(format!("Lecture du processus impossible : {error}"));
                break;
            }
        }
    }
    let _ = child.wait();
    let stdout = stdout_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    let stderr = stderr_thread
        .and_then(|thread| thread.join().ok())
        .unwrap_or_default();
    let mut output = [stdout.trim(), stderr.trim()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(ref error) = forced_error {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str(error);
    }
    if output.is_empty() {
        output = "Commande terminee sans sortie".to_string();
    }
    let exit_code = exit.as_ref().and_then(ExitStatus::code);
    ValidationResult {
        passed: exit.is_some_and(|status| status.success()) && forced_error.is_none(),
        exit_code,
        duration_ms: started.elapsed().as_millis().min(u64::MAX as u128) as u64,
        output: truncate(&output, MAX_TEST_OUTPUT_BYTES),
    }
}

fn read_capped(mut reader: impl Read) -> String {
    let mut retained = Vec::new();
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if retained.len() < MAX_TEST_OUTPUT_BYTES {
            let remaining = MAX_TEST_OUTPUT_BYTES - retained.len();
            retained.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    String::from_utf8_lossy(&retained).to_string()
}

fn team_message_author(run: &OrchestrationSnapshot, message: &OrchestrationTeamMessage) -> String {
    match message.from_role {
        OrchestrationAccountRole::Orchestrator => "Orchestrateur".to_string(),
        OrchestrationAccountRole::Worker => message
            .from_task_id
            .as_deref()
            .and_then(|id| run.tasks.iter().find(|task| task.id == id))
            .map(|task| format!("Worker {}", task.position))
            .unwrap_or_else(|| "Worker".to_string()),
    }
}

fn team_message_targets(run: &OrchestrationSnapshot, message: &OrchestrationTeamMessage) -> String {
    if message.to_task_ids.is_empty() {
        return "tout le groupe".to_string();
    }
    message
        .to_task_ids
        .iter()
        .filter_map(|id| run.tasks.iter().find(|task| task.id == *id))
        .map(|task| format!("Worker {}", task.position))
        .collect::<Vec<_>>()
        .join(", ")
}

fn team_feed(run: &OrchestrationSnapshot, task_id: Option<&str>) -> String {
    let mut messages = run
        .team_messages
        .iter()
        .rev()
        .filter(|message| {
            task_id.is_none()
                || message.to_task_ids.is_empty()
                || task_id.is_some_and(|id| message.to_task_ids.iter().any(|target| target == id))
                || message.from_task_id.as_deref() == task_id
        })
        .take(30)
        .collect::<Vec<_>>();
    messages.reverse();
    if messages.is_empty() {
        return "Aucun message pour le moment.".to_string();
    }
    messages
        .into_iter()
        .map(|message| {
            format!(
                "[#{}] {} -> {} : {}",
                message.sequence,
                team_message_author(run, message),
                team_message_targets(run, message),
                message.body
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn plan_prompt(run: &OrchestrationSnapshot) -> String {
    let retry = run
        .last_error
        .as_deref()
        .map(|error| format!("\nLe format precedent a ete refuse : {error}\n"))
        .unwrap_or_default();
    let allowed = if run.adaptive_fanout {
        adaptive_fanout_cardinalities(run.max_task_count)
            .into_iter()
            .filter(|count| *count >= run.minimum_task_count)
            .map(|count| count.to_string())
            .collect::<Vec<_>>()
            .join("/")
    } else {
        run.worker_count.to_string()
    };
    format!(
        "Tu es l'agent orchestrateur du chat orchestre \"{}\". Tu travailles dans un worktree Git prive et tu ne dois rien modifier pendant cette phase de planification.\n\nObjectif utilisateur :\n{}\n\nChoisis une cardinalite autorisee parmi {allowed}, sans jamais descendre sous le plancher {}. Utilise le maximum de missions independantes qui apportent une valeur reelle, sans doublon ni remplissage. Decoupe ensuite l'objectif en exactement cette cardinalite de taches coherentes, petites et testables. Chaque tache ouvre un chat travailleur distinct et chaque critere doit etre observable. Pour une source ephemere, produis des livrables concrets sous reports/task-XX.md et ne pretends jamais modifier le chemin utilisateur.{}\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_PLAN: {{\"summary\":\"strategie concise\",\"tasks\":[{{\"title\":\"titre\",\"description\":\"travail attendu\",\"acceptanceCriteria\":[\"critere verifiable\"]}}]}}",
        run.name, run.objective, run.minimum_task_count, retry
    )
}

fn tester_plan_prompt(run: &OrchestrationSnapshot, tester: &OrchestrationTester) -> String {
    let assigned_tasks = tester
        .assigned_task_ids
        .iter()
        .filter_map(|task_id| run.tasks.iter().find(|task| task.id == *task_id))
        .map(|task| {
            format!(
                "- {} ({}): {}\n  Criteres: {}",
                task.id,
                task.title,
                task.description,
                task.acceptance_criteria.join(" | ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Tu es l'orchestrateur testeur {} du chat orchestre \"{}\". Tu es independant des workers et tu controles au plus {} missions. Pendant cette phase, ne modifie aucun fichier : inspecte le depot et concois une strategie de tests reproductible qui prouve chaque critere et recherche les regressions.\n\nObjectif global :\n{}\n\nMissions sous ta responsabilite :\n{}\n\nCommande de validation globale imposee : `{}`. Complete-la par des tests cibles pertinents. Chaque test doit avoir un nom, une commande executable dans le depot et un resultat attendu observable. Diffuse via `messages` toute contrainte utile aux workers.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_TEST_PLAN: {{\"summary\":\"strategie de validation\",\"tests\":[{{\"name\":\"comportement controle\",\"command\":\"commande reproductible\",\"expected\":\"resultat observable\"}}],\"messages\":[{{\"toTaskIds\":[\"task-01\"],\"body\":\"contrainte de test utile\"}}]}}",
        tester.position,
        run.name,
        WORKERS_PER_TESTER,
        run.objective,
        assigned_tasks,
        run.test_command
    )
}

fn tester_validation_prompt(run: &OrchestrationSnapshot, tester: &OrchestrationTester) -> String {
    let assigned_tasks = tester
        .assigned_task_ids
        .iter()
        .filter_map(|task_id| run.tasks.iter().find(|task| task.id == *task_id))
        .map(|task| format!("- {}: {}", task.id, task.title))
        .collect::<Vec<_>>()
        .join("\n");
    let test_plan = serde_json::to_string(&tester.test_plan).unwrap_or_default();
    let previous_failure = run
        .last_error
        .as_deref()
        .map(|error| format!("\nEchec precedent a diagnostiquer :\n{error}\n"))
        .unwrap_or_default();
    format!(
        "Tu es l'orchestrateur testeur {} du chat orchestre \"{}\". Toutes les contributions actuellement acceptees sont integrees dans ce worktree prive. Tu ne dois modifier aucun fichier : inspecte le diff, execute reellement tout ton plan de tests et cherche les bugs, regressions et criteres non remplis.{}\n\nObjectif :\n{}\n\nMissions que tu controles :\n{}\n\nPlan obligatoire :\n{}\n\nTu peux ajouter des commandes de diagnostic, mais chaque test planifie doit apparaitre dans `tests`. Decision `pass` uniquement si tous les tests sont executes et reussis et si tous les criteres sont remplis. Sinon decision `revise`, avec au moins un `taskIds` parmi tes missions et un feedback actionnable : les workers correspondants continueront a corriger, puis tous les testeurs repasseront. N'accepte jamais un resultat partiel.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_TEST_RESULT: {{\"decision\":\"pass\",\"summary\":\"resultat de validation\",\"taskIds\":[],\"feedback\":\"\",\"tests\":[{{\"command\":\"commande executee\",\"result\":\"resultat observe\",\"passed\":true}}],\"messages\":[]}}",
        tester.position,
        run.name,
        previous_failure,
        run.objective,
        assigned_tasks,
        test_plan
    )
}

fn worker_prompt(run: &OrchestrationSnapshot, task: &OrchestrationTask) -> String {
    let criteria = task
        .acceptance_criteria
        .iter()
        .map(|criterion| format!("- {criterion}"))
        .collect::<Vec<_>>()
        .join("\n");
    let revision = task
        .last_error
        .as_deref()
        .map(|feedback| {
            format!(
                "\nRetour obligatoire de l'orchestrateur ou de la validation :\n{feedback}\nCorrige cette cause avant de resoumettre."
            )
        })
        .unwrap_or_default();
    let group_feed = team_feed(run, Some(&task.id));
    format!(
        "Tu es le travailleur charge de la tache {} du chat orchestre \"{}\". Ton environnement Git est isole. Respecte les changements existants et reste strictement dans le perimetre de cette tache.\n\nObjectif global :\n{}\n\nTache : {}\n{}\n\nCriteres d'acceptation :\n{}{}\n\nFil de coordination du groupe (messages pertinents, ordre croissant) :\n{}\n\nImplemente la tache, exploite les informations utiles du groupe, inspecte le diff reel et execute les tests pertinents. Tu peux transmettre jusqu'a 20 messages aux autres workers dans `messages`; une liste `toTaskIds` vide diffuse au groupe. N'y mets que des informations actionnables (contrat, chemin, risque, dependance ou resultat). Ne declare jamais une garantie absolue d'absence de bug : fournis des preuves reproductibles. Une preuve sans test reussi ou sans modification reelle sera refusee. N'effectue aucune publication ni action externe irreversible.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_PROOF: {{\"summary\":\"resultat obtenu\",\"filesChanged\":[\"chemin\"],\"tests\":[{{\"command\":\"commande executee\",\"result\":\"resultat observe\",\"passed\":true}}],\"risks\":[],\"messages\":[{{\"toTaskIds\":[\"task-02\"],\"body\":\"information utile\"}}]}}",
        task.position,
        run.name,
        run.objective,
        task.title,
        task.description,
        criteria,
        revision,
        group_feed
    )
}

fn review_prompt(run: &OrchestrationSnapshot, task: &OrchestrationTask) -> String {
    let proof = task
        .evidence
        .as_ref()
        .map(|proof| serde_json::to_string(proof).unwrap_or_default())
        .unwrap_or_default();
    format!(
        "Tu es l'agent orchestrateur. Le patch de la tache {} est applique et stage dans TON worktree prive. Inspecte `git diff --cached`, confronte-le aux criteres, execute des tests pertinents et cherche activement bugs, regressions, manque de couverture et ameliorations necessaires. Tu peux corriger de petites imperfections directement dans ton environnement ; toute correction substantielle doit etre renvoyee au travailleur.\n\nTache : {}\n{}\nCriteres :\n{}\n\nPreuve soumise :\n{}\n\nFil de coordination du groupe :\n{}\n\nDecision `accept` seulement si la contribution est propre, complete et testee. Sinon `revise` avec un retour precis et actionnable. Tu peux relayer des informations aux workers via `messages`; une liste `toTaskIds` vide diffuse au groupe. La commande globale `{}` sera executee mecaniquement apres ton acceptation.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_REVIEW: {{\"decision\":\"accept\",\"summary\":\"constat\",\"feedback\":\"\",\"tests\":[{{\"command\":\"commande executee\",\"result\":\"resultat observe\",\"passed\":true}}],\"messages\":[]}}",
        task.position,
        task.title,
        task.description,
        task.acceptance_criteria.join("\n- "),
        proof,
        team_feed(run, None),
        run.test_command
    )
}

fn merge_review_prompt(run: &OrchestrationSnapshot, pending_task_ids: &[String]) -> String {
    let pending = pending_task_ids
        .iter()
        .filter_map(|task_id| run.tasks.iter().find(|task| task.id == *task_id))
        .map(|task| {
            format!(
                "- {} (worker {}): {}\n  Criteres: {}",
                task.id,
                task.position,
                task.title,
                task.acceptance_criteria.join(" | ")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let already_reviewed = run
        .merge_reviewed_task_ids
        .iter()
        .filter_map(|task_id| run.tasks.iter().find(|task| task.id == *task_id))
        .map(|task| format!("- {} (worker {}): {}", task.id, task.position, task.title))
        .collect::<Vec<_>>()
        .join("\n");
    format!(
        "Tu es le chat orchestrateur principal. Un orchestrateur testeur vient de valider un lot de sous-chats. Avant de continuer, verifie s'il existe d'autres contributions deja terminees et controle la fusion REELLE du lot avec tout ce qui a deja ete integre. Travaille dans ton worktree prive : inspecte le diff cumule depuis le commit de base, les recouvrements de fichiers, les contrats partages et les regressions entre sous-chats. Execute des tests d'integration pertinents. Tu peux corriger directement une petite imperfection de raccord ; une correction substantielle doit etre renvoyee aux workers concernes.\n\nObjectif global :\n{}\n\nNouveau lot valide a fusionner ensemble :\n{}\n\nLots deja controles (ils restent dans le resultat combine) :\n{}\n\nFil de coordination du groupe :\n{}\n\nDecision `accept` uniquement si le resultat combine est coherent et teste. En cas de conflit ou d'incompatibilite, decision `revise` avec tous les `taskIds` concernes et un feedback actionnable. Toute correction directe declenchera automatiquement un nouveau passage des testeurs avant l'audit final.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_MERGE: {{\"decision\":\"accept\",\"summary\":\"fusion controlee\",\"taskIds\":[],\"feedback\":\"\",\"tests\":[{{\"command\":\"commande executee\",\"result\":\"resultat observe\",\"passed\":true}}],\"messages\":[]}}",
        run.objective,
        pending,
        if already_reviewed.is_empty() {
            "Aucun lot precedent.".to_string()
        } else {
            already_reviewed
        },
        team_feed(run, None)
    )
}

fn final_review_prompt(run: &OrchestrationSnapshot) -> String {
    let tasks = run
        .tasks
        .iter()
        .map(|task| format!("- {}: {}", task.id, task.title))
        .collect::<Vec<_>>()
        .join("\n");
    let previous_failure = run
        .last_error
        .as_deref()
        .map(|error| format!("\nLa derniere validation finale a echoue :\n{error}\n"))
        .unwrap_or_default();
    format!(
        "Tu es l'agent orchestrateur et toutes les contributions sont integrees dans ton environnement prive. Realise l'audit final de l'objectif complet : inspecte le diff depuis le commit de base, execute les tests, recherche les regressions et les ameliorations indispensables.{}\nObjectif :\n{}\n\nTaches disponibles pour un retour :\n{}\n\nFil de coordination du groupe :\n{}\n\nSi un probleme subsiste, choisis exactement un taskId et renvoie-le au travailleur avec un feedback actionnable. Sinon declare complete. La commande globale `{}` sera encore executee mecaniquement avant le rendu.\n\nTermine par exactement une ligne, sans bloc Markdown :\nORCHESTRATION_FINAL: {{\"decision\":\"complete\",\"summary\":\"resultat final\",\"taskId\":null,\"feedback\":\"\",\"tests\":[{{\"command\":\"commande executee\",\"result\":\"resultat observe\",\"passed\":true}}],\"messages\":[]}}",
        previous_failure,
        run.objective,
        tasks,
        team_feed(run, None),
        run.test_command
    )
}

fn snapshot_text(snapshot: &ChatTurnSnapshot) -> String {
    snapshot
        .parts
        .iter()
        .filter_map(|part| part.text.as_deref())
        .chain(
            snapshot
                .thoughts
                .iter()
                .map(|thought| thought.text.as_str()),
        )
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_marked_json<T: for<'de> Deserialize<'de>>(text: &str, marker: &str) -> Result<T, String> {
    let payload = text
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix(marker).map(str::trim))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("ligne {marker} absente"))?;
    serde_json::from_str(payload).map_err(|error| format!("JSON invalide : {error}"))
}

fn validate_plan(
    mut plan: PlanEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<PlanEnvelope, String> {
    plan.summary = validate_short_text(&plan.summary, "Le resume du plan")?;
    if plan.tasks.is_empty() {
        return Err("le plan ne contient aucune tache".to_string());
    }
    let task_count = plan.tasks.len() as u32;
    if run.adaptive_fanout {
        if !adaptive_fanout_cardinalities(run.max_task_count).contains(&task_count) {
            return Err(format!(
                "le plan adaptatif doit utiliser entre 1 et {} taches, mais il en contient {task_count}",
                run.max_task_count
            ));
        }
        if task_count < run.minimum_task_count {
            return Err(format!(
                "le plan contient {task_count} taches, sous le plancher persistant de {}",
                run.minimum_task_count
            ));
        }
    } else if task_count != run.worker_count {
        return Err(format!(
            "le plan doit contenir exactement {} tache{} (une par worker), mais il en contient {}",
            run.worker_count,
            if run.worker_count > 1 { "s" } else { "" },
            plan.tasks.len()
        ));
    }
    let mut signatures = HashSet::new();
    for task in &mut plan.tasks {
        task.title = validate_short_text(&task.title, "Le titre d'une tache")?;
        task.description = validate_short_text(&task.description, "La description d'une tache")?;
        task.acceptance_criteria = task
            .acceptance_criteria
            .drain(..)
            .map(|value| validate_short_text(&value, "Un critere d'acceptation"))
            .collect::<Result<Vec<_>, _>>()?;
        if task.acceptance_criteria.is_empty() {
            return Err(format!("la tache '{}' n'a aucun critere", task.title));
        }
        if task_count >= 20 {
            let signature = format!(
                "{}\n{}",
                task.title.to_lowercase(),
                task.description.to_lowercase()
            );
            if !signatures.insert(signature) {
                return Err(
                    "le plan massif contient des taches dupliquees servant de remplissage"
                        .to_string(),
                );
            }
        }
    }
    Ok(plan)
}

fn validate_team_messages(
    messages: &mut Vec<TeamMessageEnvelope>,
    run: &OrchestrationSnapshot,
) -> Result<(), String> {
    if messages.len() > MAX_TEAM_MESSAGES_PER_TURN {
        return Err(format!(
            "un tour ne peut pas publier plus de {MAX_TEAM_MESSAGES_PER_TURN} messages de groupe"
        ));
    }
    let known_tasks = run
        .tasks
        .iter()
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    for message in messages {
        message.body = validate_required_text(
            &message.body,
            MAX_TEAM_MESSAGE_CHARS,
            "Le message de groupe",
        )?;
        let mut seen = HashSet::new();
        message.to_task_ids = message
            .to_task_ids
            .drain(..)
            .map(|target| target.trim().to_string())
            .filter(|target| !target.is_empty() && seen.insert(target.clone()))
            .collect();
        if let Some(unknown) = message
            .to_task_ids
            .iter()
            .find(|target| !known_tasks.contains(target.as_str()))
        {
            return Err(format!("destinataire de groupe inconnu : {unknown}"));
        }
    }
    Ok(())
}

fn append_team_messages(
    run: &mut OrchestrationSnapshot,
    from_role: OrchestrationAccountRole,
    from_task_id: Option<&str>,
    messages: Vec<TeamMessageEnvelope>,
    now: i64,
) {
    let mut sequence = run
        .team_messages
        .last()
        .map(|message| message.sequence.saturating_add(1))
        .unwrap_or(1);
    for message in messages {
        run.team_messages.push(OrchestrationTeamMessage {
            id: Uuid::new_v4().to_string(),
            sequence,
            timestamp: now,
            from_role,
            from_task_id: from_task_id.map(str::to_string),
            to_task_ids: message.to_task_ids,
            body: message.body,
        });
        sequence = sequence.saturating_add(1);
    }
    if run.team_messages.len() > MAX_TEAM_MESSAGES {
        run.team_messages
            .drain(0..run.team_messages.len() - MAX_TEAM_MESSAGES);
    }
}

fn validate_proof(mut proof: ProofEnvelope) -> Result<ProofEnvelope, String> {
    proof.summary = validate_short_text(&proof.summary, "Le resume de la preuve")?;
    if proof.tests.is_empty() {
        return Err("aucun test n'est fourni".to_string());
    }
    if proof.tests.iter().any(|test| !test.passed) {
        return Err("au moins un test soumis est en echec".to_string());
    }
    for test in &mut proof.tests {
        test.command = validate_short_text(&test.command, "La commande de test")?;
        test.result = validate_short_text(&test.result, "Le resultat de test")?;
    }
    proof.files_changed = proof
        .files_changed
        .into_iter()
        .filter_map(|value| normalize_optional(Some(value)))
        .collect();
    proof.risks = proof
        .risks
        .into_iter()
        .filter_map(|value| normalize_optional(Some(value)))
        .map(|value| truncate(&value, 2_000))
        .collect();
    Ok(proof)
}

fn validate_tester_plan(mut plan: TesterPlanEnvelope) -> Result<TesterPlanEnvelope, String> {
    plan.summary = validate_short_text(&plan.summary, "Le resume du plan de tests")?;
    if plan.tests.is_empty() {
        return Err("le testeur doit definir au moins un test".to_string());
    }
    for test in &mut plan.tests {
        test.name = validate_short_text(&test.name, "Le nom du test")?;
        test.command = validate_short_text(&test.command, "La commande du test")?;
        test.expected = validate_short_text(&test.expected, "Le resultat attendu")?;
    }
    Ok(plan)
}

fn validate_tester_result(
    mut result: TesterResultEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<TesterResultEnvelope, String> {
    result.summary = validate_short_text(&result.summary, "Le resume du testeur")?;
    result.feedback = truncate(result.feedback.trim(), MAX_TEXT_CHARS);
    if result.tests.is_empty() {
        return Err("le testeur doit fournir les resultats des tests".to_string());
    }
    for test in &mut result.tests {
        test.command = validate_short_text(&test.command, "La commande de test")?;
        test.result = validate_short_text(&test.result, "Le resultat de test")?;
    }
    let tester_id = run
        .current_tester_id
        .as_deref()
        .ok_or_else(|| "orchestrateur testeur courant absent".to_string())?;
    let tester = run
        .testers
        .iter()
        .find(|tester| tester.id == tester_id)
        .ok_or_else(|| format!("orchestrateur testeur inconnu : {tester_id}"))?;
    if let Some(missing) = tester.test_plan.iter().find(|planned| {
        !result
            .tests
            .iter()
            .any(|executed| executed.command.trim() == planned.command.trim())
    }) {
        return Err(format!(
            "le test planifie '{}' n'a pas ete execute",
            missing.name
        ));
    }
    let assigned = tester
        .assigned_task_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    result.task_ids = result
        .task_ids
        .drain(..)
        .map(|task_id| task_id.trim().to_string())
        .filter(|task_id| !task_id.is_empty() && seen.insert(task_id.clone()))
        .collect();
    if let Some(unknown) = result
        .task_ids
        .iter()
        .find(|task_id| !assigned.contains(task_id.as_str()))
    {
        return Err(format!(
            "le testeur ne peut rouvrir que ses missions ; taskId interdit : {unknown}"
        ));
    }
    match result.decision {
        OrchestrationTesterDecision::Pass => {
            if result.tests.iter().any(|test| !test.passed) {
                return Err("un testeur ne peut pas valider avec un test en echec".to_string());
            }
            if !result.task_ids.is_empty() || !result.feedback.is_empty() {
                return Err("une validation reussie ne doit pas demander de correction".to_string());
            }
        }
        OrchestrationTesterDecision::Revise => {
            if result.task_ids.is_empty() {
                return Err("une correction doit cibler au moins un worker".to_string());
            }
            if result.feedback.is_empty() {
                return Err("une correction doit fournir un feedback actionnable".to_string());
            }
        }
    }
    Ok(result)
}

fn validate_review(mut review: ReviewEnvelope) -> Result<ReviewEnvelope, String> {
    review.summary = validate_short_text(&review.summary, "Le resume de la revue")?;
    review.feedback = truncate(review.feedback.trim(), MAX_TEXT_CHARS);
    if review.decision == OrchestrationReviewDecision::Accept {
        if review.tests.is_empty() {
            return Err("une acceptation sans test est interdite".to_string());
        }
        if review.tests.iter().any(|test| !test.passed) {
            return Err("une acceptation contient un test en echec".to_string());
        }
    } else if review.feedback.is_empty() {
        return Err("une revision doit contenir un feedback".to_string());
    }
    Ok(review)
}

fn validate_merge_review(
    mut review: MergeReviewEnvelope,
    run: &OrchestrationSnapshot,
    pending_task_ids: &[String],
) -> Result<MergeReviewEnvelope, String> {
    review.summary = validate_short_text(&review.summary, "Le resume de la fusion")?;
    review.feedback = truncate(review.feedback.trim(), MAX_TEXT_CHARS);
    if review.tests.is_empty() {
        return Err("la revue de fusion exige au moins un test execute".to_string());
    }
    for test in &mut review.tests {
        test.command = validate_short_text(&test.command, "La commande de fusion")?;
        test.result = validate_short_text(&test.result, "Le resultat de fusion")?;
    }

    let accepted = run
        .tasks
        .iter()
        .filter(|task| task.status == OrchestrationTaskStatus::Accepted)
        .map(|task| task.id.as_str())
        .collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    review.task_ids = review
        .task_ids
        .drain(..)
        .map(|task_id| task_id.trim().to_string())
        .filter(|task_id| !task_id.is_empty() && seen.insert(task_id.clone()))
        .collect();
    if let Some(unknown) = review
        .task_ids
        .iter()
        .find(|task_id| !accepted.contains(task_id.as_str()))
    {
        return Err(format!(
            "la fusion ne peut rouvrir qu'une mission acceptee ; taskId interdit : {unknown}"
        ));
    }

    match review.decision {
        OrchestrationReviewDecision::Accept => {
            if review.tests.iter().any(|test| !test.passed) {
                return Err("une fusion acceptee contient un test en echec".to_string());
            }
            if !review.task_ids.is_empty() || !review.feedback.is_empty() {
                return Err("une fusion acceptee ne doit pas demander de correction".to_string());
            }
            if pending_task_ids.is_empty() {
                return Err("aucune mission validee n'attend la fusion".to_string());
            }
        }
        OrchestrationReviewDecision::Revise => {
            if review.task_ids.is_empty() {
                return Err("une correction de fusion doit cibler au moins un worker".to_string());
            }
            if review.feedback.is_empty() {
                return Err(
                    "une correction de fusion doit fournir un feedback actionnable".to_string(),
                );
            }
        }
    }
    Ok(review)
}

fn validate_final(
    mut final_review: FinalEnvelope,
    run: &OrchestrationSnapshot,
) -> Result<FinalEnvelope, String> {
    final_review.summary = validate_short_text(&final_review.summary, "Le resume final")?;
    final_review.feedback = truncate(final_review.feedback.trim(), MAX_TEXT_CHARS);
    match final_review.decision {
        FinalDecision::Complete => {
            if final_review.tests.is_empty() || final_review.tests.iter().any(|test| !test.passed) {
                return Err("la conclusion finale exige au moins un test reussi".to_string());
            }
            final_review.task_id = None;
        }
        FinalDecision::Revise => {
            let task_id = final_review
                .task_id
                .as_deref()
                .ok_or_else(|| "taskId absent pour la revision".to_string())?;
            if !run.tasks.iter().any(|task| task.id == task_id) {
                return Err(format!("taskId inconnu : {task_id}"));
            }
            if final_review.feedback.is_empty() {
                return Err("le feedback de revision est vide".to_string());
            }
        }
    }
    Ok(final_review)
}

fn next_open_task(run: &OrchestrationSnapshot) -> Option<&OrchestrationTask> {
    if let Some(current_id) = run.current_task_id.as_deref() {
        if let Some(task) = run
            .tasks
            .iter()
            .find(|task| task.id == current_id && task.status != OrchestrationTaskStatus::Accepted)
        {
            return Some(task);
        }
    }
    run.tasks
        .iter()
        .find(|task| task.status != OrchestrationTaskStatus::Accepted)
}

fn assign_session(
    run: &mut OrchestrationSnapshot,
    kind: OrchestrationTurnKind,
    task_id: Option<&str>,
    tester_id: Option<&str>,
    session_id: String,
) -> Result<(), String> {
    if session_id.trim().is_empty() {
        return Ok(());
    }
    if kind == OrchestrationTurnKind::Worker {
        let id = task_id.ok_or_else(|| "Tache absente pour la session".to_string())?;
        find_task_mut(run, id)?.session_id = Some(session_id);
    } else if matches!(
        kind,
        OrchestrationTurnKind::TesterPlan | OrchestrationTurnKind::TesterValidation
    ) {
        let id =
            tester_id.ok_or_else(|| "Orchestrateur testeur absent pour la session".to_string())?;
        find_tester_mut(run, id)?.session_id = Some(session_id);
    } else {
        run.orchestrator_session_id = Some(session_id);
    }
    Ok(())
}

fn clear_current_turn(
    run: &mut OrchestrationSnapshot,
    kind: OrchestrationTurnKind,
    snapshot: &ChatTurnSnapshot,
) -> Result<(), String> {
    run.current_turn_id = None;
    run.current_turn_kind = None;
    run.current_start_id = None;
    if let Some(session_id) = snapshot.session_id.clone() {
        let task_id = run.current_task_id.clone();
        let tester_id = run.current_tester_id.clone();
        assign_session(
            run,
            kind,
            task_id.as_deref(),
            tester_id.as_deref(),
            session_id,
        )?;
    }
    Ok(())
}

fn recover_phase_for_resume(run: &mut OrchestrationSnapshot, now: i64) {
    run.current_turn_id = None;
    run.current_turn_kind = None;
    run.current_start_id = None;
    run.current_validation_id = None;
    run.current_validation_kind = None;
    match run.phase {
        OrchestrationPhase::DesigningTests => {
            if let Some(tester_id) = run.current_tester_id.clone() {
                if let Ok(tester) = find_tester_mut(run, &tester_id) {
                    tester.status = OrchestrationTesterStatus::Pending;
                }
            }
        }
        OrchestrationPhase::Reviewing | OrchestrationPhase::Validating => {
            if let Some(task_id) = run.current_task_id.clone() {
                if let Ok(task) = find_task_mut(run, &task_id) {
                    task.status = OrchestrationTaskStatus::RevisionRequested;
                    task.last_error = Some(
                        "Cycle de revue interrompu ; verifie le travail et resoumets la preuve"
                            .to_string(),
                    );
                }
            }
            run.phase = OrchestrationPhase::Working;
        }
        OrchestrationPhase::Testing => {
            if let Some(tester_id) = run.current_tester_id.clone() {
                if let Ok(tester) = find_tester_mut(run, &tester_id) {
                    tester.status = OrchestrationTesterStatus::Ready;
                }
            }
        }
        OrchestrationPhase::FinalValidation => {
            reset_testers_for_validation(
                run,
                Some("Validation finale interrompue ; nouveau passage obligatoire"),
            );
            run.phase = OrchestrationPhase::Testing;
        }
        OrchestrationPhase::Completed => {}
        _ => {}
    }
    run.current_tester_id = None;
    run.next_action_at = Some(now);
}

fn prepare_source_repository(
    raw: &str,
    sandbox_root: &Path,
    authorized_boundary: Option<&str>,
) -> Result<PreparedSourceRepository, String> {
    let authorized_boundary = authorized_boundary
        .map(|raw| {
            let boundary = fs::canonicalize(raw.trim()).map_err(|_| {
                "La frontiere autorisee de l'environnement est introuvable".to_string()
            })?;
            if !boundary.is_dir() {
                return Err(
                    "La frontiere autorisee de l'environnement n'est pas un dossier".to_string(),
                );
            }
            Ok(boundary)
        })
        .transpose()?;
    let requested_project_dir = normalize_optional(Some(truncate(raw.trim(), MAX_TEXT_CHARS)));
    if let Some(requested) = requested_project_dir.as_deref() {
        if let Ok((repository, base_commit)) =
            inspect_source_repository_with_boundary(requested, authorized_boundary.as_deref())
        {
            let source_kind = match git_text(
                &repository,
                [
                    "-c",
                    "core.fsmonitor=false",
                    "status",
                    "--porcelain",
                    "--untracked-files=all",
                ],
            ) {
                Ok(status) if status.trim().is_empty() => OrchestrationSourceKind::GitClean,
                Ok(_) | Err(_) => OrchestrationSourceKind::GitDirty,
            };
            if source_kind == OrchestrationSourceKind::GitDirty {
                if let Ok(private_repository) =
                    clone_private_source_repository(&repository, &base_commit, sandbox_root)
                {
                    return Ok(PreparedSourceRepository {
                        repository: private_repository,
                        base_commit,
                        source_kind,
                        requested_project_dir,
                        access_project_dir: Some(repository.to_string_lossy().to_string()),
                    });
                }
                return create_ephemeral_prepared_source(sandbox_root, requested_project_dir);
            }
            return Ok(PreparedSourceRepository {
                access_project_dir: Some(repository.to_string_lossy().to_string()),
                repository,
                base_commit,
                source_kind,
                requested_project_dir,
            });
        }
    }
    create_ephemeral_prepared_source(sandbox_root, requested_project_dir)
}

fn create_ephemeral_prepared_source(
    sandbox_root: &Path,
    requested_project_dir: Option<String>,
) -> Result<PreparedSourceRepository, String> {
    let repository = create_ephemeral_source_repository(sandbox_root)?;
    let base_commit = git_text(&repository, ["rev-parse", "HEAD"])?;
    Ok(PreparedSourceRepository {
        repository,
        base_commit,
        source_kind: OrchestrationSourceKind::Ephemeral,
        requested_project_dir,
        access_project_dir: None,
    })
}

fn clone_private_source_repository(
    source: &Path,
    base_commit: &str,
    sandbox_root: &Path,
) -> Result<PathBuf, String> {
    let repository = sandbox_root.join("source");
    let mut command = git_command();
    add_git_safe_directory(&mut command, source);
    let output = command
        .args(["clone", "--no-local", "--no-checkout", "--no-tags"])
        .arg(source)
        .arg(&repository)
        .output()
        .map_err(|error| format!("Git est indisponible : {error}"))?;
    if !output.status.success() {
        let _ = fs::remove_dir_all(&repository);
        return Err(command_error(
            "Clone prive du depot source sale impossible",
            &output,
        ));
    }
    if git_text(
        &repository,
        ["rev-parse", &format!("{base_commit}^{{commit}}")],
    )
    .is_err()
    {
        let _ = fs::remove_dir_all(&repository);
        return Err("Commit HEAD absent du clone prive".to_string());
    }
    let _ = run_git(&repository, ["remote", "remove", "origin"], None);
    Ok(repository)
}

fn create_ephemeral_source_repository(sandbox_root: &Path) -> Result<PathBuf, String> {
    let repository = sandbox_root.join("source");
    if repository.exists() {
        fs::remove_dir_all(&repository)
            .map_err(|error| format!("Nettoyage du repli prive impossible : {error}"))?;
    }
    fs::create_dir_all(&repository).map_err(|error| {
        format!(
            "Creation du depot prive de repli impossible ({}): {error}",
            repository.display()
        )
    })?;
    run_git(&repository, ["init"], None)?;
    let context = serde_json::json!({
        "schemaVersion": 1,
        "workspaceMode": "ephemeral",
        "sourceCopied": false,
        "notice": "Depot prive volontairement vide; aucun chemin demande n'a ete copie ni modifie."
    });
    fs::write(
        repository.join(".switch-orchestration-context.json"),
        serde_json::to_vec_pretty(&context).map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("Ecriture du contexte prive impossible : {error}"))?;
    run_git(&repository, ["add", "-A"], None)?;
    run_git(
        &repository,
        [
            "-c",
            "user.name=Codex Switch Orchestrator",
            "-c",
            "user.email=orchestrator@codex-switch.local",
            "-c",
            "commit.gpgSign=false",
            "-c",
            &format!("core.hooksPath={}", disabled_git_hooks_path()),
            "commit",
            "--no-gpg-sign",
            "-m",
            "Initialize private orchestration workspace",
        ],
        None,
    )?;
    Ok(repository)
}

fn inspect_source_repository(raw: &str) -> Result<(PathBuf, String), String> {
    inspect_source_repository_with_boundary(raw, None)
}

fn inspect_source_repository_with_boundary(
    raw: &str,
    authorized_boundary: Option<&Path>,
) -> Result<(PathBuf, String), String> {
    let path = Path::new(raw.trim());
    if !path.is_dir() {
        return Err("Le dossier projet n'existe pas".to_string());
    }
    let path = fs::canonicalize(path).map_err(|_| "Le dossier projet n'existe pas".to_string())?;
    if authorized_boundary.is_some_and(|boundary| !path.starts_with(boundary)) {
        return Err("Le dossier projet depasse la frontiere autorisee".to_string());
    }
    let mut candidate = path.clone();
    let git_candidate = loop {
        if candidate.join(".git").exists() {
            break Some(candidate.clone());
        }
        if authorized_boundary.is_some_and(|boundary| candidate == boundary) {
            break None;
        }
        let Some(parent) = candidate.parent() else {
            break None;
        };
        if authorized_boundary.is_some_and(|boundary| !parent.starts_with(boundary)) {
            break None;
        }
        candidate = parent.to_path_buf();
    }
    .ok_or_else(|| "Le dossier projet n'est pas un depot Git utilisable".to_string())?;

    let root_text = git_text(&git_candidate, ["rev-parse", "--show-toplevel"])
        .map_err(|_| "Le dossier projet n'est pas un depot Git utilisable".to_string())?;
    let root = fs::canonicalize(PathBuf::from(root_text.trim()))
        .map_err(|_| "La racine Git est introuvable".to_string())?;
    if authorized_boundary.is_some_and(|boundary| !root.starts_with(boundary)) {
        return Err("La racine Git depasse la frontiere autorisee".to_string());
    }
    let git_dir_text = git_text(&root, ["rev-parse", "--absolute-git-dir"])
        .map_err(|_| "Les metadonnees Git sont introuvables".to_string())?;
    let git_dir = fs::canonicalize(PathBuf::from(git_dir_text.trim()))
        .map_err(|_| "Les metadonnees Git sont introuvables".to_string())?;
    if authorized_boundary.is_some_and(|boundary| !git_dir.starts_with(boundary)) {
        return Err("Les metadonnees Git depassent la frontiere autorisee".to_string());
    }
    let common_dir_text = git_text(
        &root,
        ["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map_err(|_| "Les metadonnees Git communes sont introuvables".to_string())?;
    let common_dir = fs::canonicalize(PathBuf::from(common_dir_text.trim()))
        .map_err(|_| "Les metadonnees Git communes sont introuvables".to_string())?;
    if authorized_boundary.is_some_and(|boundary| !common_dir.starts_with(boundary)) {
        return Err("Les metadonnees Git communes depassent la frontiere autorisee".to_string());
    }
    for objects_dir in [git_dir.join("objects"), common_dir.join("objects")] {
        let alternates = objects_dir.join("info").join("alternates");
        let Ok(content) = fs::read_to_string(&alternates) else {
            continue;
        };
        for line in content
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
        {
            let candidate = PathBuf::from(line);
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                objects_dir.join(candidate)
            };
            let resolved = fs::canonicalize(candidate)
                .map_err(|_| "Un depot d'objets Git alternatif est introuvable".to_string())?;
            if authorized_boundary.is_some_and(|boundary| !resolved.starts_with(boundary)) {
                return Err(
                    "Un depot d'objets Git alternatif depasse la frontiere autorisee".to_string(),
                );
            }
        }
    }
    let base_commit = git_text(&root, ["rev-parse", "HEAD"])
        .map_err(|_| "Le depot Git ne contient aucun commit utilisable".to_string())?;
    Ok((root, base_commit))
}

fn ensure_clean_repository(path: &Path) -> Result<(), String> {
    let status = git_text(path, ["status", "--porcelain", "--untracked-files=all"])?;
    if status.trim().is_empty() {
        Ok(())
    } else {
        Err(
            "Le depot doit etre propre avant de demarrer ou de publier un chat orchestre. Commit ou stash les changements presents."
                .to_string(),
        )
    }
}

fn add_worktree(repo: &Path, target: &Path, commit: &str) -> Result<(), String> {
    if target.exists() {
        return Err(format!("Le sandbox existe deja : {}", target.display()));
    }
    let output = git_command_at(repo)
        .args(["worktree", "add", "--detach"])
        .arg(target)
        .arg(commit)
        .output()
        .map_err(|error| format!("Git est indisponible : {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("Creation du worktree impossible", &output))
    }
}

#[cfg(windows)]
fn disabled_git_hooks_path() -> &'static str {
    "NUL"
}

#[cfg(not(windows))]
fn disabled_git_hooks_path() -> &'static str {
    "/dev/null"
}

fn reset_owned_worktree(path: &Path, commit: &str) -> Result<(), String> {
    if !path.is_dir() {
        return Err(format!("Sandbox introuvable : {}", path.display()));
    }
    run_git(path, ["reset", "--hard", commit], None)?;
    run_git(path, ["clean", "-ffdx"], None)
}

fn stage_and_changed_files(path: &Path, base: &str) -> Result<Vec<String>, String> {
    run_git(path, ["add", "-A"], None)?;
    let output = git_text(path, ["diff", "--cached", "--name-only", base])?;
    Ok(output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.replace('\\', "/"))
        .collect())
}

fn remove_owned_worktrees(
    run: &OrchestrationSnapshot,
    trusted_sandboxes_path: &Path,
) -> Result<(), String> {
    let repo = Path::new(&run.project_dir);
    Uuid::parse_str(&run.id)
        .map_err(|_| "Identifiant de sandbox invalide ; nettoyage refuse".to_string())?;
    let trusted_root = fs::canonicalize(trusted_sandboxes_path)
        .map_err(|error| format!("Racine des sandboxes introuvable : {error}"))?;
    let expected_root = comparison_path(&trusted_root.join(&run.id));
    let root = comparison_path(Path::new(&run.sandbox_root));
    if root != expected_root || root == trusted_root || !root.starts_with(&trusted_root) {
        return Err("Refus de nettoyer un dossier hors de la racine des sandboxes".to_string());
    }
    let mut paths = run
        .tasks
        .iter()
        .filter_map(|task| task.workspace_dir.as_deref())
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    let workers_root = root.join("workers");
    if let Ok(entries) = fs::read_dir(&workers_root) {
        paths.extend(
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir()),
        );
    }
    paths.push(PathBuf::from(&run.orchestrator_dir));
    paths.sort();
    paths.dedup();
    for path in paths {
        let comparable = comparison_path(&path);
        if !comparable.starts_with(&root) {
            return Err("Refus de supprimer un worktree hors du sandbox".to_string());
        }
        if path.exists() {
            let output = git_command_at(repo)
                .args(["worktree", "remove", "--force"])
                .arg(&path)
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(command_error("Suppression du worktree impossible", &output));
            }
        }
    }
    let _ = run_git(repo, ["worktree", "prune"], None);
    if root.exists() {
        fs::remove_dir_all(root).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn comparison_path(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| {
        path.parent()
            .and_then(|parent| fs::canonicalize(parent).ok())
            .and_then(|parent| path.file_name().map(|name| parent.join(name)))
            .unwrap_or_else(|| path.to_path_buf())
    })
}

fn git_command() -> Command {
    let mut command = Command::new("git");
    command
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", disabled_git_hooks_path())
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .arg("-c")
        .arg(format!("core.hooksPath={}", disabled_git_hooks_path()))
        .args(["-c", "core.fsmonitor=false"]);
    command
}

fn git_safe_directory(path: &Path) -> String {
    let normalized = comparison_path(path).to_string_lossy().replace('\\', "/");
    if let Some(unc) = normalized.strip_prefix("//?/UNC/") {
        format!("//{unc}")
    } else {
        normalized
            .strip_prefix("//?/")
            .unwrap_or(&normalized)
            .to_string()
    }
}

fn add_git_safe_directory(command: &mut Command, path: &Path) {
    command
        .arg("-c")
        .arg(format!("safe.directory={}", git_safe_directory(path)));
}

fn git_command_at(path: &Path) -> Command {
    let mut command = git_command();
    add_git_safe_directory(&mut command, path);
    command.arg("-C").arg(path);
    command
}

fn git_text<'a>(path: &Path, args: impl IntoIterator<Item = &'a str>) -> Result<String, String> {
    let output = git_command_at(path)
        .args(args)
        .output()
        .map_err(|error| format!("Git est indisponible : {error}"))?;
    if !output.status.success() {
        return Err(command_error("Commande Git echouee", &output));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn git_status<'a>(
    path: &Path,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<ExitStatus, String> {
    git_command_at(path)
        .args(args)
        .status()
        .map_err(|error| format!("Git est indisponible : {error}"))
}

fn git_output_file<'a>(
    path: &Path,
    args: impl IntoIterator<Item = &'a str>,
    output_path: &Path,
) -> Result<(), String> {
    let file = File::create(output_path).map_err(|error| error.to_string())?;
    let output = git_command_at(path)
        .args(args)
        .stdout(Stdio::from(file))
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Git est indisponible : {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("Creation du patch impossible", &output))
    }
}

fn run_git<'a>(
    path: &Path,
    args: impl IntoIterator<Item = &'a str>,
    stdin_path: Option<&Path>,
) -> Result<(), String> {
    let mut command = git_command_at(path);
    command.args(args);
    if let Some(stdin_path) = stdin_path {
        command.stdin(Stdio::from(
            File::open(stdin_path).map_err(|error| error.to_string())?,
        ));
    }
    let output = command
        .output()
        .map_err(|error| format!("Git est indisponible : {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_error("Commande Git echouee", &output))
    }
}

fn command_error(label: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    if detail.is_empty() {
        format!("{label} (code {:?})", output.status.code())
    } else {
        format!("{label} : {}", truncate(&detail, MAX_TEXT_CHARS))
    }
}

fn shell_command(command_text: &str) -> Command {
    #[cfg(windows)]
    {
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/S", "/C", command_text]);
        command
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("sh");
        command.args(["-lc", command_text]);
        command.process_group(0);
        command
    }
}

fn terminate_process_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let mut command = Command::new("taskkill");
        command.args(["/PID", &child.id().to_string(), "/T", "/F"]);
        hide_process_window(&mut command);
        let _ = command.status();
    }
    #[cfg(not(windows))]
    {
        let _ = Command::new("kill")
            .args(["-KILL", &format!("-{}", child.id())])
            .status();
    }
    let _ = child.kill();
}

#[cfg(windows)]
fn hide_process_window(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    command.creation_flags(0x08000000);
}

#[cfg(not(windows))]
fn hide_process_window(_command: &mut Command) {}

fn find_run_mut<'a>(
    store: &'a mut OrchestrationStore,
    id: &str,
) -> Result<&'a mut OrchestrationSnapshot, String> {
    store
        .runs
        .iter_mut()
        .find(|run| run.id == id)
        .ok_or_else(|| "Orchestration introuvable".to_string())
}

fn find_task_mut<'a>(
    run: &'a mut OrchestrationSnapshot,
    id: &str,
) -> Result<&'a mut OrchestrationTask, String> {
    run.tasks
        .iter_mut()
        .find(|task| task.id == id)
        .ok_or_else(|| "Tache orchestree introuvable".to_string())
}

fn find_tester_mut<'a>(
    run: &'a mut OrchestrationSnapshot,
    id: &str,
) -> Result<&'a mut OrchestrationTester, String> {
    run.testers
        .iter_mut()
        .find(|tester| tester.id == id)
        .ok_or_else(|| "Orchestrateur testeur introuvable".to_string())
}

fn push_event(run: &mut OrchestrationSnapshot, timestamp: i64, kind: &str, message: String) {
    run.events.push(OrchestrationEvent {
        timestamp,
        kind: kind.to_string(),
        message: truncate(&message, 2_000),
    });
    if run.events.len() > MAX_EVENTS {
        run.events.drain(0..run.events.len() - MAX_EVENTS);
    }
}

fn validate_required_text(value: &str, max: usize, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} est vide"));
    }
    if value.len() > max {
        return Err(format!("{label} depasse la taille autorisee"));
    }
    Ok(value.to_string())
}

fn validate_short_text(value: &str, label: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{label} est vide"));
    }
    if value.chars().count() > MAX_TEXT_CHARS {
        return Err(format!("{label} est trop long"));
    }
    Ok(value.to_string())
}

fn validate_name(value: Option<&str>, objective: &str) -> Result<String, String> {
    let name = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            objective
                .lines()
                .next()
                .unwrap_or("Chat orchestre")
                .to_string()
        });
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(format!("Le nom depasse {MAX_NAME_CHARS} caracteres"));
    }
    Ok(name.chars().take(MAX_NAME_CHARS).collect())
}

fn validate_test_timeout(value: u64) -> Result<u64, String> {
    if !(MIN_TEST_TIMEOUT_SECONDS..=MAX_TEST_TIMEOUT_SECONDS).contains(&value) {
        return Err(format!(
            "Le timeout doit etre compris entre {MIN_TEST_TIMEOUT_SECONDS} et {MAX_TEST_TIMEOUT_SECONDS} secondes"
        ));
    }
    Ok(value)
}

fn validate_worker_count(value: u32) -> Result<u32, String> {
    let limit = configured_worker_limit();
    if !(MIN_WORKER_COUNT..=limit).contains(&value) {
        return Err(format!(
            "Le nombre de workers doit etre compris entre {MIN_WORKER_COUNT} et {limit}, sans compter l'orchestrateur"
        ));
    }
    Ok(value)
}

fn required_tester_count(worker_count: u32) -> u32 {
    worker_count.max(1).div_ceil(WORKERS_PER_TESTER)
}

fn build_testers(tasks: &[OrchestrationTask]) -> Vec<OrchestrationTester> {
    let tester_count = required_tester_count(tasks.len() as u32);
    (0..tester_count)
        .map(|index| {
            let start = (index * WORKERS_PER_TESTER) as usize;
            let end = ((index + 1) * WORKERS_PER_TESTER) as usize;
            OrchestrationTester {
                id: format!("tester-{:02}", index + 1),
                position: index + 1,
                status: OrchestrationTesterStatus::Pending,
                assigned_task_ids: tasks[start.min(tasks.len())..end.min(tasks.len())]
                    .iter()
                    .map(|task| task.id.clone())
                    .collect(),
                session_id: None,
                plan_summary: None,
                test_plan: Vec::new(),
                last_results: Vec::new(),
                attempt_count: 0,
                protocol_failures: 0,
                last_error: None,
            }
        })
        .collect()
}

fn testers_all_passed(run: &OrchestrationSnapshot) -> bool {
    run.tester_count > 0
        && run.testers.len() == run.tester_count as usize
        && run
            .testers
            .iter()
            .all(|tester| tester.status == OrchestrationTesterStatus::Passed)
}

fn pending_merge_review_task_ids(run: &OrchestrationSnapshot) -> Vec<String> {
    let passed_task_ids = run
        .testers
        .iter()
        .filter(|tester| tester.status == OrchestrationTesterStatus::Passed)
        .flat_map(|tester| tester.assigned_task_ids.iter().map(String::as_str))
        .collect::<HashSet<_>>();
    let reviewed_task_ids = run
        .merge_reviewed_task_ids
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    run.tasks
        .iter()
        .filter(|task| {
            task.status == OrchestrationTaskStatus::Accepted
                && passed_task_ids.contains(task.id.as_str())
                && !reviewed_task_ids.contains(task.id.as_str())
        })
        .map(|task| task.id.clone())
        .collect()
}

fn apply_merge_review_acceptance(
    run: &mut OrchestrationSnapshot,
    task_ids: &[String],
    integrated_commit: String,
    integration_changed: bool,
    summary: &str,
    now: i64,
) -> Result<(), String> {
    if task_ids.is_empty() {
        return Err("Aucune mission validee a marquer comme fusionnee".to_string());
    }
    let pending = pending_merge_review_task_ids(run)
        .into_iter()
        .collect::<HashSet<_>>();
    if let Some(task_id) = task_ids.iter().find(|task_id| !pending.contains(*task_id)) {
        return Err(format!(
            "Mission absente du lot de fusion courant : {task_id}"
        ));
    }
    for task_id in task_ids {
        if !run.merge_reviewed_task_ids.contains(task_id) {
            run.merge_reviewed_task_ids.push(task_id.clone());
        }
    }
    run.integrated_commit = integrated_commit;
    run.protocol_failures = 0;
    run.current_task_id = None;
    run.current_tester_id = None;
    run.last_error = None;
    run.updated_at = now;

    if integration_changed {
        reset_testers_for_validation(
            run,
            Some("Le chat orchestrateur a ajuste la fusion ; les tests doivent repasser"),
        );
        run.phase = OrchestrationPhase::Testing;
    } else {
        run.phase = if testers_all_passed(run) {
            OrchestrationPhase::FinalReview
        } else {
            OrchestrationPhase::Testing
        };
    }
    run.next_action_at = Some(now);
    push_event(
        run,
        now,
        if integration_changed {
            "merge_review_updated"
        } else {
            "merge_review_passed"
        },
        format!(
            "{} sous-chat{} fusionne{} et controle{} par l'orchestrateur : {}",
            task_ids.len(),
            if task_ids.len() > 1 { "s" } else { "" },
            if task_ids.len() > 1 { "s" } else { "" },
            if task_ids.len() > 1 { "s" } else { "" },
            truncate(summary, 1_000)
        ),
    );
    Ok(())
}

fn reset_testers_for_validation(run: &mut OrchestrationSnapshot, feedback: Option<&str>) {
    for tester in &mut run.testers {
        tester.status = OrchestrationTesterStatus::Ready;
        tester.last_error = feedback.map(|value| truncate(value, MAX_TEXT_CHARS));
    }
}

fn default_adaptive_fanout() -> bool {
    true
}

fn default_max_task_count() -> u32 {
    DEFAULT_MAX_TASK_COUNT
}

fn default_max_concurrency() -> u32 {
    DEFAULT_MAX_CONCURRENCY
}

fn validate_max_task_count(value: u32) -> Result<u32, String> {
    if !(MIN_TASK_COUNT..=MAX_TASK_COUNT).contains(&value) {
        return Err(format!(
            "Le plafond de taches doit etre compris entre {MIN_TASK_COUNT} et {MAX_TASK_COUNT}"
        ));
    }
    Ok(value)
}

fn validate_max_concurrency(value: u32) -> Result<u32, String> {
    if !(MIN_MAX_CONCURRENCY..=MAX_MAX_CONCURRENCY).contains(&value) {
        return Err(format!(
            "La concurrence doit etre comprise entre {MIN_MAX_CONCURRENCY} et {MAX_MAX_CONCURRENCY}"
        ));
    }
    Ok(value)
}

fn adaptive_fanout_cardinalities(max_task_count: u32) -> Vec<u32> {
    (MIN_TASK_COUNT..=max_task_count.min(MAX_TASK_COUNT)).collect()
}

fn objective_requests_fanout(objective: &str, requested: u32) -> bool {
    let normalized = objective
        .to_lowercase()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>();
    let tokens = normalized.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        if token.parse::<u32>().ok() != Some(requested) {
            continue;
        }
        let before = &tokens[index.saturating_sub(5)..index];
        if before.iter().any(|token| {
            matches!(
                *token,
                "pas" | "sans" | "jamais" | "eviter" | "evite" | "interdit"
            )
        }) {
            continue;
        }
        let after = &tokens[index + 1..(index + 4).min(tokens.len())];
        if before
            .iter()
            .any(|token| matches!(*token, "par" | "x" | "exactement"))
            || after.iter().any(|token| {
                matches!(
                    *token,
                    "agent" | "agents" | "worker" | "workers" | "tache" | "taches"
                )
            })
        {
            return true;
        }
    }
    false
}

fn objective_fanout_floor(objective: &str, max_task_count: u32) -> u32 {
    (2..=max_task_count.min(MAX_TASK_COUNT))
        .rev()
        .find(|requested| objective_requests_fanout(objective, *requested))
        .unwrap_or(1)
}

fn requested_minimum_task_count(
    objective: &str,
    worker_count: u32,
    adaptive_fanout: bool,
    max_task_count: u32,
) -> u32 {
    if adaptive_fanout {
        worker_count.max(objective_fanout_floor(objective, max_task_count))
    } else {
        worker_count
    }
}

fn configured_default_worker_count() -> u32 {
    let configured = std::env::var(WORKER_COUNT_ENV).ok();
    configured_default_worker_count_from(configured.as_deref())
}

fn configured_default_worker_count_from(configured: Option<&str>) -> u32 {
    let limit = configured_worker_limit();
    configured
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| (MIN_WORKER_COUNT..=limit).contains(value))
        .unwrap_or(DEFAULT_WORKER_COUNT.min(limit))
}

fn configured_worker_limit() -> u32 {
    let configured = std::env::var(WORKER_LIMIT_ENV).ok();
    configured_worker_limit_from(configured.as_deref())
}

fn configured_worker_limit_from(configured: Option<&str>) -> u32 {
    configured
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| (MIN_WORKER_COUNT..=MAX_WORKER_COUNT).contains(value))
        .unwrap_or(DEFAULT_WORKER_LIMIT)
}

fn configured_default_max_concurrency() -> u32 {
    let configured = std::env::var(CONCURRENCY_ENV).ok();
    configured_default_max_concurrency_from(configured.as_deref())
}

fn configured_default_max_concurrency_from(configured: Option<&str>) -> u32 {
    configured
        .and_then(|value| value.trim().parse::<u32>().ok())
        .filter(|value| (MIN_MAX_CONCURRENCY..=MAX_MAX_CONCURRENCY).contains(value))
        .unwrap_or(DEFAULT_MAX_CONCURRENCY)
}

/// Emplacement des sandboxes ephemeres des agents (RAM-disk via
/// `CST_ORCHESTRATION_SANDBOX_DIR` si defini, sinon a cote de l'etat persiste).
fn configured_sandboxes_path(storage_path: &Path) -> PathBuf {
    sandboxes_path_from_env(storage_path, std::env::var_os(SANDBOX_DIR_ENV).as_deref())
}

fn sandboxes_path_from_env(storage_path: &Path, configured: Option<&OsStr>) -> PathBuf {
    configured
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            storage_path
                .parent()
                .unwrap_or_else(|| Path::new("."))
                .join("orchestrated-runs")
        })
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn short_id(value: &str) -> String {
    value.chars().take(8).collect()
}

fn normalize_loaded_store(store: &mut OrchestrationStore, now: i64) -> bool {
    let mut changed = store.version != STORE_VERSION;
    store.version = STORE_VERSION;
    for run in &mut store.runs {
        if run.worker_count == 0 {
            run.worker_count = if run.tasks.is_empty() {
                DEFAULT_WORKER_COUNT
            } else {
                (run.tasks.len() as u32).clamp(MIN_WORKER_COUNT, MAX_WORKER_COUNT)
            };
            changed = true;
        }
        if !(MIN_TASK_COUNT..=MAX_TASK_COUNT).contains(&run.max_task_count) {
            run.max_task_count = DEFAULT_MAX_TASK_COUNT;
            changed = true;
        }
        if run.tasks.len() <= MAX_TASK_COUNT as usize && run.max_task_count < run.tasks.len() as u32
        {
            run.max_task_count = run.tasks.len() as u32;
            changed = true;
        }
        if !(MIN_TASK_COUNT..=run.max_task_count).contains(&run.minimum_task_count) {
            run.minimum_task_count = if run.tasks.is_empty() {
                requested_minimum_task_count(
                    &run.objective,
                    run.worker_count,
                    run.adaptive_fanout,
                    run.max_task_count,
                )
            } else {
                run.tasks.len() as u32
            }
            .min(run.max_task_count);
            changed = true;
        }
        if !(MIN_MAX_CONCURRENCY..=MAX_MAX_CONCURRENCY).contains(&run.max_concurrency) {
            run.max_concurrency = DEFAULT_MAX_CONCURRENCY;
            changed = true;
        }
        if run.source_kind == OrchestrationSourceKind::GitClean
            && run.access_project_dir.is_none()
            && !run.project_dir.trim().is_empty()
        {
            run.access_project_dir = Some(run.project_dir.clone());
            changed = true;
        }
        if run.orchestrator_account_id.trim().is_empty() {
            run.orchestrator_account_id = run.account_id.clone();
            changed = true;
        }
        if run.worker_account_ids.len() != run.worker_count as usize {
            let previous = run.worker_account_ids.clone();
            run.worker_account_ids = (0..run.worker_count)
                .map(|position| {
                    previous
                        .get(position as usize)
                        .filter(|value| !value.trim().is_empty())
                        .cloned()
                        .or_else(|| {
                            run.tasks
                                .iter()
                                .find(|task| task.position == position + 1)
                                .map(|task| task.account_id.clone())
                                .filter(|value| !value.trim().is_empty())
                        })
                        .unwrap_or_else(|| run.account_id.clone())
                })
                .collect();
            changed = true;
        }
        for task in &mut run.tasks {
            if task.account_id.trim().is_empty() {
                task.account_id = run
                    .worker_account_ids
                    .get(task.position.saturating_sub(1) as usize)
                    .cloned()
                    .unwrap_or_else(|| run.account_id.clone());
                changed = true;
            }
        }
        let expected_tester_count = required_tester_count(run.worker_count);
        if run.tester_count != expected_tester_count {
            run.tester_count = expected_tester_count;
            changed = true;
        }
        if !run.tasks.is_empty() && run.testers.len() != expected_tester_count as usize {
            let previous = std::mem::take(&mut run.testers);
            let completed = run.status == OrchestrationStatus::Completed;
            run.testers = build_testers(&run.tasks)
                .into_iter()
                .map(|mut tester| {
                    if let Some(existing) = previous.iter().find(|item| item.id == tester.id) {
                        tester.session_id = existing.session_id.clone();
                        tester.plan_summary = existing.plan_summary.clone();
                        tester.test_plan = existing.test_plan.clone();
                        tester.last_results = existing.last_results.clone();
                        tester.attempt_count = existing.attempt_count;
                        tester.protocol_failures = existing.protocol_failures;
                        tester.last_error = existing.last_error.clone();
                        tester.status = existing.status;
                    } else if completed {
                        tester.status = OrchestrationTesterStatus::Passed;
                    }
                    tester
                })
                .collect();
            changed = true;
        }
        let accepted_task_ids = run
            .tasks
            .iter()
            .filter(|task| task.status == OrchestrationTaskStatus::Accepted)
            .map(|task| task.id.clone())
            .collect::<HashSet<_>>();
        let mut seen_merge_reviews = HashSet::new();
        let previous_merge_review_count = run.merge_reviewed_task_ids.len();
        run.merge_reviewed_task_ids.retain(|task_id| {
            accepted_task_ids.contains(task_id) && seen_merge_reviews.insert(task_id.clone())
        });
        if run.merge_reviewed_task_ids.len() != previous_merge_review_count {
            changed = true;
        }
        if run.status != OrchestrationStatus::Completed
            && matches!(
                run.phase,
                OrchestrationPhase::FinalReview
                    | OrchestrationPhase::FinalValidation
                    | OrchestrationPhase::Publishing
            )
            && !testers_all_passed(run)
        {
            run.phase = if run.testers.iter().any(|tester| tester.test_plan.is_empty()) {
                OrchestrationPhase::DesigningTests
            } else {
                OrchestrationPhase::Testing
            };
            changed = true;
        }
        if run.status != OrchestrationStatus::Completed
            && matches!(
                run.phase,
                OrchestrationPhase::Testing
                    | OrchestrationPhase::FinalReview
                    | OrchestrationPhase::FinalValidation
                    | OrchestrationPhase::Publishing
            )
            && !pending_merge_review_task_ids(run).is_empty()
        {
            run.phase = OrchestrationPhase::Merging;
            changed = true;
        }
        if run.team_messages.len() > MAX_TEAM_MESSAGES {
            run.team_messages
                .drain(0..run.team_messages.len() - MAX_TEAM_MESSAGES);
            changed = true;
        }
        let was_active = run.status == OrchestrationStatus::Active;
        let interrupted = run.current_turn_id.take().is_some()
            || run.current_start_id.take().is_some()
            || run.current_validation_id.take().is_some();
        run.current_turn_kind = None;
        run.current_validation_kind = None;
        if was_active {
            if interrupted {
                recover_phase_for_resume(run, now);
                run.last_error = Some(
                    "Execution interrompue par le redemarrage ; reprise automatique en cours"
                        .to_string(),
                );
            } else {
                run.next_action_at = Some(now);
            }
            run.status = OrchestrationStatus::Active;
            run.next_action_at = Some(now);
            push_event(
                run,
                now,
                "recovered",
                if interrupted {
                    "Interruption detectee ; orchestration reprise automatiquement".to_string()
                } else {
                    "Orchestration active reprise au redemarrage".to_string()
                },
            );
            changed = true;
        } else if interrupted {
            recover_phase_for_resume(run, now);
            run.next_action_at = None;
            run.last_error = Some(
                "Execution interrompue par le redemarrage ; l'orchestration reste arretee"
                    .to_string(),
            );
            push_event(
                run,
                now,
                "recovered",
                "Interruption detectee ; statut utilisateur conserve".to_string(),
            );
            changed = true;
        }
    }
    changed
}

fn load_store(path: &Path) -> Result<OrchestrationStore, String> {
    if !path.exists() {
        return Ok(OrchestrationStore::default());
    }
    let content = fs::read_to_string(path).map_err(|error| error.to_string())?;
    match serde_json::from_str::<OrchestrationStore>(&content) {
        Ok(store) if store.version <= STORE_VERSION => Ok(store),
        Ok(store) => Err(format!(
            "Version d'etat des orchestrations non supportee : {}",
            store.version
        )),
        Err(error) => {
            let backup = path.with_extension(format!("corrupt-{}.json", metrics::now_ts()));
            fs::rename(path, &backup).map_err(|rename_error| {
                format!(
                    "Etat des orchestrations illisible ({error}) et sauvegarde impossible ({rename_error})"
                )
            })?;
            Ok(OrchestrationStore::default())
        }
    }
}

fn persist_store(path: &Path, store: &OrchestrationStore) -> Result<(), String> {
    let content = serde_json::to_string_pretty(store).map_err(|error| error.to_string())?;
    fs_util::atomic_write(path, content).map_err(|error| error.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn list_orchestrations(
    state: State<'_, OrchestrationManager>,
) -> Result<Vec<OrchestrationSnapshot>, String> {
    state.list()
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn create_orchestration(
    state: State<'_, OrchestrationManager>,
    request: CreateOrchestrationRequest,
) -> Result<OrchestrationSnapshot, String> {
    state.create(request)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn promote_autonomous_agent_to_orchestration(
    orchestration: State<'_, OrchestrationManager>,
    autonomous: State<'_, AutonomousAgentManager>,
    id: String,
    request: PromoteAutonomousAgentRequest,
) -> Result<OrchestrationSnapshot, String> {
    orchestration.promote_autonomous_agent(&autonomous, &id, request)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn control_orchestration(
    state: State<'_, OrchestrationManager>,
    id: String,
    action: OrchestrationAction,
) -> Result<OrchestrationSnapshot, String> {
    state.control(&id, action)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn reassign_orchestration_account(
    state: State<'_, OrchestrationManager>,
    id: String,
    request: ReassignOrchestrationAccountRequest,
) -> Result<OrchestrationSnapshot, String> {
    state.reassign_account(&id, request)
}

#[cfg(feature = "desktop")]
#[tauri::command]
pub fn delete_orchestration(
    state: State<'_, OrchestrationManager>,
    id: String,
) -> Result<(), String> {
    state.delete(&id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_orchestration_rejects_the_interactive_freebuff_provider() {
        assert!(!orchestration_provider_supported(settings::Provider::Freebuff));
        for provider in [
            settings::Provider::Codex,
            settings::Provider::Claude,
            settings::Provider::OpenCode,
            settings::Provider::Aihubmix,
            settings::Provider::OpenAiCompatible,
        ] {
            assert!(orchestration_provider_supported(provider));
        }
    }

    #[test]
    fn git_commands_trust_only_the_exact_repository() {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("le manifeste Rust doit appartenir au depot");
        let expected = format!("safe.directory={}", git_safe_directory(repository));
        let command = git_command_at(repository);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().to_string())
            .collect::<Vec<_>>();

        assert!(arguments.iter().any(|argument| argument == &expected));
        assert!(!arguments
            .iter()
            .any(|argument| argument == "safe.directory=*"));
        assert!(!git_text(repository, ["rev-parse", "HEAD"])
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parses_single_line_plan_contract() {
        let mut run = sample_run();
        run.adaptive_fanout = false;
        let plan = parse_marked_json::<PlanEnvelope>(
            "analyse\nORCHESTRATION_PLAN: {\"summary\":\"Plan\",\"tasks\":[{\"title\":\"API\",\"description\":\"Ajouter l'API\",\"acceptanceCriteria\":[\"Le test passe\"]}]}",
            "ORCHESTRATION_PLAN:",
        )
        .and_then(|plan| validate_plan(plan, &run))
        .unwrap();
        assert_eq!(plan.tasks.len(), 1);
        assert_eq!(plan.tasks[0].title, "API");
    }

    #[test]
    fn plan_must_match_the_selected_worker_count() {
        let mut run = sample_run();
        run.adaptive_fanout = false;
        run.worker_count = 2;
        let plan = PlanEnvelope {
            summary: "Plan".to_string(),
            tasks: vec![PlanTask {
                title: "API".to_string(),
                description: "Ajouter l'API".to_string(),
                acceptance_criteria: vec!["Le test passe".to_string()],
            }],
        };
        let error = validate_plan(plan, &run).unwrap_err();
        assert!(error.contains("exactement 2 taches"));
        assert!(validate_worker_count(0).is_err());
        assert!(validate_worker_count(MAX_WORKER_COUNT + 1).is_err());
    }

    #[test]
    fn configured_worker_count_uses_only_valid_environment_fallbacks() {
        assert_eq!(configured_worker_limit_from(None), 5);
        assert_eq!(configured_worker_limit_from(Some("1000")), 1000);
        assert_eq!(configured_worker_limit_from(Some("1001")), 5);
        assert_eq!(configured_default_worker_count_from(None), 5);
        assert_eq!(configured_default_worker_count_from(Some("4")), 4);
        assert_eq!(configured_default_worker_count_from(Some("0")), 5);
        assert_eq!(configured_default_worker_count_from(Some("8")), 5);
        assert_eq!(configured_default_worker_count_from(Some("abc")), 5);
    }

    #[test]
    fn tester_ratio_is_one_per_five_workers_with_a_minimum_of_one() {
        assert_eq!(required_tester_count(0), 1);
        assert_eq!(required_tester_count(1), 1);
        assert_eq!(required_tester_count(5), 1);
        assert_eq!(required_tester_count(6), 2);
        assert_eq!(required_tester_count(10), 2);
        assert_eq!(required_tester_count(11), 3);
        assert_eq!(required_tester_count(1_000), 200);
    }

    #[test]
    fn testers_receive_disjoint_batches_of_at_most_five_tasks() {
        let mut sample = sample_run();
        let template = sample.tasks.remove(0);
        let tasks = (1..=11)
            .map(|position| {
                let mut task = template.clone();
                task.id = format!("task-{position:02}");
                task.position = position;
                task
            })
            .collect::<Vec<_>>();
        let testers = build_testers(&tasks);
        assert_eq!(testers.len(), 3);
        assert_eq!(testers[0].assigned_task_ids.len(), 5);
        assert_eq!(testers[1].assigned_task_ids.len(), 5);
        assert_eq!(testers[2].assigned_task_ids, vec!["task-11"]);
        let assigned = testers
            .iter()
            .flat_map(|tester| tester.assigned_task_ids.iter())
            .collect::<HashSet<_>>();
        assert_eq!(assigned.len(), tasks.len());
    }

    #[test]
    fn tester_cannot_pass_until_every_planned_test_succeeds() {
        let mut run = sample_run();
        run.current_tester_id = Some("tester-01".to_string());
        let missing = TesterResultEnvelope {
            decision: OrchestrationTesterDecision::Pass,
            summary: "Validation".to_string(),
            task_ids: Vec::new(),
            feedback: String::new(),
            tests: vec![OrchestrationProofTest {
                command: "cargo check".to_string(),
                result: "ok".to_string(),
                passed: true,
            }],
            messages: Vec::new(),
        };
        assert!(validate_tester_result(missing, &run).is_err());

        let failed = TesterResultEnvelope {
            decision: OrchestrationTesterDecision::Pass,
            summary: "Validation".to_string(),
            task_ids: Vec::new(),
            feedback: String::new(),
            tests: vec![OrchestrationProofTest {
                command: "cargo test".to_string(),
                result: "failed".to_string(),
                passed: false,
            }],
            messages: Vec::new(),
        };
        assert!(validate_tester_result(failed, &run).is_err());

        let passed = TesterResultEnvelope {
            decision: OrchestrationTesterDecision::Pass,
            summary: "Validation".to_string(),
            task_ids: Vec::new(),
            feedback: String::new(),
            tests: vec![OrchestrationProofTest {
                command: "cargo test".to_string(),
                result: "ok".to_string(),
                passed: true,
            }],
            messages: Vec::new(),
        };
        assert!(validate_tester_result(passed, &run).is_ok());
    }

    #[test]
    fn passed_tester_batches_wait_for_an_orchestrator_merge_review() {
        let mut run = sample_run();
        let mut second = run.tasks[0].clone();
        second.id = "task-02".to_string();
        second.position = 2;
        second.title = "Interface".to_string();
        run.tasks.push(second);
        run.worker_count = 2;
        run.testers[0].assigned_task_ids = vec!["task-01".to_string(), "task-02".to_string()];

        let pending = pending_merge_review_task_ids(&run);
        assert_eq!(pending, vec!["task-01", "task-02"]);
        apply_merge_review_acceptance(
            &mut run,
            &pending,
            "merged-head".to_string(),
            false,
            "Les deux sous-chats sont compatibles",
            123,
        )
        .unwrap();

        assert_eq!(run.merge_reviewed_task_ids, vec!["task-01", "task-02"]);
        assert_eq!(run.integrated_commit, "merged-head");
        assert_eq!(run.phase, OrchestrationPhase::FinalReview);
        assert!(pending_merge_review_task_ids(&run).is_empty());
    }

    #[test]
    fn an_orchestrator_merge_fix_forces_every_tester_to_run_again() {
        let mut run = sample_run();
        let pending = pending_merge_review_task_ids(&run);
        apply_merge_review_acceptance(
            &mut run,
            &pending,
            "fixed-merge-head".to_string(),
            true,
            "Raccord corrige par l'orchestrateur",
            456,
        )
        .unwrap();

        assert_eq!(run.phase, OrchestrationPhase::Testing);
        assert_eq!(run.testers[0].status, OrchestrationTesterStatus::Ready);
        assert_eq!(run.merge_reviewed_task_ids, vec!["task-01"]);
        assert!(pending_merge_review_task_ids(&run).is_empty());
        assert!(run
            .events
            .iter()
            .any(|event| event.kind == "merge_review_updated"));
    }

    #[test]
    fn merge_review_rejects_an_unproven_or_unknown_revision() {
        let run = sample_run();
        let pending = pending_merge_review_task_ids(&run);
        let failed_accept = MergeReviewEnvelope {
            decision: OrchestrationReviewDecision::Accept,
            summary: "Fusion".to_string(),
            task_ids: Vec::new(),
            feedback: String::new(),
            tests: vec![OrchestrationProofTest {
                command: "cargo test".to_string(),
                result: "failed".to_string(),
                passed: false,
            }],
            messages: Vec::new(),
        };
        assert!(validate_merge_review(failed_accept, &run, &pending).is_err());

        let unknown_revision = MergeReviewEnvelope {
            decision: OrchestrationReviewDecision::Revise,
            summary: "Conflit".to_string(),
            task_ids: vec!["task-99".to_string()],
            feedback: "Corriger le contrat partage".to_string(),
            tests: vec![OrchestrationProofTest {
                command: "cargo test".to_string(),
                result: "failed".to_string(),
                passed: false,
            }],
            messages: Vec::new(),
        };
        assert!(validate_merge_review(unknown_revision, &run, &pending).is_err());
    }

    #[test]
    fn failed_tester_reopens_workers_and_invalidates_the_whole_quorum() {
        let mut run = sample_run();
        let mut second = run.tasks[0].clone();
        second.id = "task-02".to_string();
        second.position = 2;
        second.title = "Interface".to_string();
        run.tasks.push(second);
        run.worker_count = 2;
        run.testers[0].assigned_task_ids = vec!["task-01".to_string(), "task-02".to_string()];
        run.testers[0].status = OrchestrationTesterStatus::Passed;
        run.merge_reviewed_task_ids = vec!["task-01".to_string(), "task-02".to_string()];
        run.phase = OrchestrationPhase::Testing;

        let reopened = vec!["task-01".to_string(), "task-02".to_string()];
        let titles = apply_tester_revision_state(
            &mut run,
            &reopened,
            "Les tests de regression echouent",
            123,
        )
        .unwrap();

        assert_eq!(titles, vec!["API", "Interface"]);
        assert!(run.tasks.iter().all(|task| {
            task.status == OrchestrationTaskStatus::RevisionRequested
                && task.last_error.as_deref() == Some("Les tests de regression echouent")
        }));
        assert_eq!(run.testers[0].status, OrchestrationTesterStatus::Ready);
        assert!(run.merge_reviewed_task_ids.is_empty());
        assert_eq!(run.phase, OrchestrationPhase::Working);
        assert_eq!(
            next_open_task(&run).map(|task| task.id.as_str()),
            Some("task-01")
        );
        assert_eq!(run.next_action_at, Some(123));
    }

    #[test]
    fn old_final_review_is_returned_to_mandatory_tester_design() {
        let mut run = sample_run();
        run.tester_count = 0;
        run.testers.clear();
        run.phase = OrchestrationPhase::FinalReview;
        let mut store = OrchestrationStore {
            version: STORE_VERSION - 1,
            runs: vec![run],
        };
        assert!(normalize_loaded_store(&mut store, 100));
        assert_eq!(store.runs[0].tester_count, 1);
        assert_eq!(store.runs[0].testers.len(), 1);
        assert_eq!(store.runs[0].phase, OrchestrationPhase::DesigningTests);
        assert!(!testers_all_passed(&store.runs[0]));
    }

    #[test]
    fn configured_concurrency_uses_only_valid_environment_fallbacks() {
        assert_eq!(configured_default_max_concurrency_from(None), 8);
        assert_eq!(configured_default_max_concurrency_from(Some("1")), 1);
        assert_eq!(configured_default_max_concurrency_from(Some("16")), 16);
        assert_eq!(configured_default_max_concurrency_from(Some("1000")), 1000);
        assert_eq!(configured_default_max_concurrency_from(Some("0")), 8);
        assert_eq!(configured_default_max_concurrency_from(Some("1001")), 8);
        assert_eq!(configured_default_max_concurrency_from(Some("abc")), 8);
    }

    #[test]
    fn sandboxes_default_next_to_state_and_can_move_to_a_ram_disk() {
        let storage = Path::new("/data/orchestrated-runs.json");
        assert_eq!(
            sandboxes_path_from_env(storage, None),
            Path::new("/data/orchestrated-runs")
        );
        assert_eq!(
            sandboxes_path_from_env(storage, Some(OsStr::new("/dev/shm/cst-agents"))),
            Path::new("/dev/shm/cst-agents")
        );
        // Valeur vide = defaut (le SSD), pas un chemin vide casse.
        assert_eq!(
            sandboxes_path_from_env(storage, Some(OsStr::new(""))),
            Path::new("/data/orchestrated-runs")
        );
    }

    #[test]
    fn global_concurrency_budget_counts_in_flight_turns_across_active_runs() {
        let mut store = OrchestrationStore::default();
        let mut in_flight = sample_run();
        in_flight.status = OrchestrationStatus::Active;
        in_flight.current_turn_id = Some(42);
        store.runs.push(in_flight.clone());

        let mut starting = sample_run();
        starting.id = "run-2".to_string();
        starting.status = OrchestrationStatus::Active;
        starting.current_start_id = Some("start".to_string());
        store.runs.push(starting);

        let mut idle = sample_run();
        idle.id = "run-3".to_string();
        idle.status = OrchestrationStatus::Active;
        store.runs.push(idle);

        let mut paused = sample_run();
        paused.id = "run-4".to_string();
        paused.status = OrchestrationStatus::Paused;
        paused.current_turn_id = Some(7);
        store.runs.push(paused);

        assert_eq!(in_flight_turns(&store), 2);
        assert!(concurrency_budget_reached_with(&store, 2));
        assert!(concurrency_budget_reached_with(&store, 1));
        assert!(!concurrency_budget_reached_with(&store, 3));
        assert!(!concurrency_budget_reached_with(&store, 8));
    }

    #[test]
    fn rejects_proof_without_successful_test() {
        let proof = ProofEnvelope {
            summary: "Implementation".to_string(),
            files_changed: vec!["src/lib.rs".to_string()],
            tests: vec![OrchestrationProofTest {
                command: "cargo test".to_string(),
                result: "failed".to_string(),
                passed: false,
            }],
            risks: Vec::new(),
            messages: Vec::new(),
        };
        assert!(validate_proof(proof).is_err());
    }

    #[test]
    fn final_revision_must_target_known_worker() {
        let run = sample_run();
        let review = FinalEnvelope {
            decision: FinalDecision::Revise,
            summary: "Bug trouve".to_string(),
            task_id: Some("task-99".to_string()),
            feedback: "Corriger".to_string(),
            tests: Vec::new(),
            messages: Vec::new(),
        };
        assert!(validate_final(review, &run).is_err());
    }

    #[test]
    fn team_messages_are_targeted_bounded_and_persisted() {
        let mut run = sample_run();
        let mut messages = vec![TeamMessageEnvelope {
            to_task_ids: vec!["task-01".to_string(), "task-01".to_string()],
            body: "Contrat API disponible".to_string(),
        }];
        validate_team_messages(&mut messages, &run).unwrap();
        assert_eq!(messages[0].to_task_ids, vec!["task-01"]);
        append_team_messages(
            &mut run,
            OrchestrationAccountRole::Worker,
            Some("task-01"),
            messages,
            123,
        );
        assert_eq!(run.team_messages.len(), 1);
        assert_eq!(run.team_messages[0].sequence, 1);
        assert!(team_feed(&run, Some("task-01")).contains("Contrat API disponible"));
    }

    #[test]
    fn active_orchestration_resumes_automatically_after_restart() {
        let mut active = sample_run();
        active.worker_count = 0;
        active.orchestrator_account_id.clear();
        active.worker_account_ids.clear();
        active.tasks[0].account_id.clear();
        active.phase = OrchestrationPhase::Working;
        active.current_turn_id = Some(42);
        active.current_turn_kind = Some(OrchestrationTurnKind::Worker);
        active.current_task_id = Some("task-01".to_string());
        active.tasks[0].status = OrchestrationTaskStatus::Working;
        let mut paused = active.clone();
        paused.id = "run-paused".to_string();
        paused.status = OrchestrationStatus::Paused;
        paused.current_turn_id = Some(43);
        let mut store = OrchestrationStore {
            version: 1,
            runs: vec![active, paused],
        };

        assert!(normalize_loaded_store(&mut store, 100));
        assert_eq!(store.version, STORE_VERSION);
        assert_eq!(store.runs[0].worker_count, 1);
        assert_eq!(store.runs[0].orchestrator_account_id, "account-1");
        assert_eq!(store.runs[0].worker_account_ids, vec!["account-1"]);
        assert_eq!(store.runs[0].tasks[0].account_id, "account-1");
        assert_eq!(store.runs[0].status, OrchestrationStatus::Active);
        assert_eq!(store.runs[0].current_turn_id, None);
        assert_eq!(store.runs[0].next_action_at, Some(100));
        assert!(store.runs[0]
            .events
            .iter()
            .any(|event| event.kind == "recovered"));
        assert_eq!(store.runs[1].status, OrchestrationStatus::Paused);
        assert_eq!(store.runs[1].current_turn_id, None);
        assert_eq!(store.runs[1].next_action_at, None);
    }

    #[test]
    fn restart_keeps_a_ready_review_and_its_feedback() {
        let mut run = sample_run();
        run.phase = OrchestrationPhase::Reviewing;
        run.current_task_id = Some("task-01".to_string());
        run.tasks[0].status = OrchestrationTaskStatus::Submitted;
        run.last_error = Some("contexte a conserver".to_string());
        run.next_action_at = Some(500);
        let mut store = OrchestrationStore {
            version: STORE_VERSION,
            runs: vec![run],
        };

        assert!(normalize_loaded_store(&mut store, 100));
        assert_eq!(store.runs[0].phase, OrchestrationPhase::Reviewing);
        assert_eq!(
            store.runs[0].tasks[0].status,
            OrchestrationTaskStatus::Submitted
        );
        assert_eq!(
            store.runs[0].last_error.as_deref(),
            Some("contexte a conserver")
        );
        assert_eq!(store.runs[0].next_action_at, Some(100));
    }

    #[test]
    fn run_level_protocol_failures_stop_after_three_attempts() {
        let dir =
            std::env::temp_dir().join(format!("cst-orchestration-protocol-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let run = sample_run();
        let inner = Arc::new(OrchestrationInner {
            chat: ChatTurnManager::default(),
            storage_path: dir.join("orchestrated-runs.json"),
            sandboxes_path: dir.join("sandboxes"),
            store: Mutex::new(OrchestrationStore {
                version: STORE_VERSION,
                runs: vec![run],
            }),
            validation_runs: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(()),
            run_locks: Mutex::new(HashMap::new()),
        });

        for attempt in 1..=MAX_PROTOCOL_FAILURES {
            protocol_failure(&inner, "run-1", None, "JSON invalide".to_string());
            let store = inner.store.lock().unwrap();
            assert_eq!(store.runs[0].protocol_failures, attempt);
            if attempt < MAX_PROTOCOL_FAILURES {
                assert_eq!(store.runs[0].status, OrchestrationStatus::Active);
            }
        }
        let store = inner.store.lock().unwrap();
        assert_eq!(store.runs[0].status, OrchestrationStatus::NeedsAttention);
        assert_eq!(store.runs[0].consecutive_start_failures, 0);
        drop(store);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_state_transition_is_rolled_back_in_memory() {
        let dir =
            std::env::temp_dir().join(format!("cst-orchestration-rollback-{}", Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let inner = OrchestrationInner {
            chat: ChatTurnManager::default(),
            storage_path: dir.join("orchestrated-runs.json"),
            sandboxes_path: dir.join("sandboxes"),
            store: Mutex::new(OrchestrationStore {
                version: STORE_VERSION,
                runs: vec![sample_run()],
            }),
            validation_runs: Mutex::new(HashMap::new()),
            lifecycle: Mutex::new(()),
            run_locks: Mutex::new(HashMap::new()),
        };

        let result: Result<(), String> = inner.mutate_store(|store| {
            store.runs[0].status = OrchestrationStatus::Completed;
            Err("transition refusee".to_string())
        });

        assert!(result.is_err());
        assert_eq!(
            inner.store.lock().unwrap().runs[0].status,
            OrchestrationStatus::Active
        );
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn git_sandboxes_merge_two_finished_workers_before_publishing() {
        let root = std::env::temp_dir().join(format!("cst-orchestration-test-{}", Uuid::new_v4()));
        let repo = root.join("repo");
        let sandboxes = root.join("sandboxes");
        let run_id = Uuid::new_v4().to_string();
        let sandbox = sandboxes.join(&run_id);
        let orchestrator = sandbox.join("orchestrator");
        let worker = sandbox.join("workers").join("task-01-01");
        let worker_two = sandbox.join("workers").join("task-02-01");
        fs::create_dir_all(&repo).unwrap();
        run_git(&repo, ["init"], None).unwrap();
        fs::write(repo.join("feature.txt"), "base\n").unwrap();
        run_git(&repo, ["add", "-A"], None).unwrap();
        run_git(
            &repo,
            [
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-m",
                "base",
            ],
            None,
        )
        .unwrap();
        let (_, base) = inspect_source_repository(repo.to_str().unwrap()).unwrap();
        fs::create_dir_all(worker.parent().unwrap()).unwrap();
        add_worktree(&repo, &orchestrator, &base).unwrap();
        add_worktree(&repo, &worker, &base).unwrap();
        add_worktree(&repo, &worker_two, &base).unwrap();
        fs::write(worker.join("feature.txt"), "base\nworker change\n").unwrap();
        fs::write(worker.join("new-file.txt"), "proof\n").unwrap();
        fs::write(worker_two.join("second-worker.txt"), "second proof\n").unwrap();

        let mut run = sample_run();
        run.id = run_id;
        run.project_dir = repo.to_string_lossy().to_string();
        run.base_commit = base.clone();
        run.integrated_commit = base.clone();
        run.sandbox_root = sandbox.to_string_lossy().to_string();
        run.orchestrator_dir = orchestrator.to_string_lossy().to_string();
        run.tasks[0].workspace_dir = Some(worker.to_string_lossy().to_string());
        run.tasks[0].base_commit = Some(base.clone());
        run.tasks[0].workspace_generation = 1;

        let changed = stage_and_changed_files(&worker, &base).unwrap();
        assert_eq!(changed, vec!["feature.txt", "new-file.txt"]);
        apply_worker_candidate(&run, &run.tasks[0]).unwrap();
        assert_eq!(
            fs::read_to_string(orchestrator.join("feature.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "base\nworker change\n"
        );
        assert_eq!(
            fs::read_to_string(orchestrator.join("new-file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "proof\n"
        );
        run.integrated_commit = commit_owned_worktree(&run, "orchestration worker one").unwrap();
        let mut second_task = run.tasks[0].clone();
        second_task.id = "task-02".to_string();
        second_task.position = 2;
        second_task.title = "Second worker".to_string();
        second_task.workspace_dir = Some(worker_two.to_string_lossy().to_string());
        second_task.base_commit = Some(base.clone());
        run.tasks.push(second_task);
        apply_worker_candidate(&run, &run.tasks[1]).unwrap();
        run.integrated_commit = commit_owned_worktree(&run, "orchestration worker two").unwrap();
        assert_eq!(
            fs::read_to_string(orchestrator.join("feature.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "base\nworker change\n"
        );
        assert_eq!(
            fs::read_to_string(orchestrator.join("second-worker.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "second proof\n"
        );
        fs::write(repo.join("source-changed.txt"), "do not overwrite\n").unwrap();
        assert!(apply_final_patch(&run).is_err());
        fs::remove_file(repo.join("source-changed.txt")).unwrap();
        apply_final_patch(&run).unwrap();
        // Simule un crash entre `git apply` et la persistance de l'etat final :
        // le second passage doit reconnaitre le rendu exact deja present.
        apply_final_patch(&run).unwrap();
        assert_eq!(
            fs::read_to_string(repo.join("feature.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "base\nworker change\n"
        );
        assert_eq!(
            fs::read_to_string(repo.join("new-file.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "proof\n"
        );
        assert_eq!(
            fs::read_to_string(repo.join("second-worker.txt"))
                .unwrap()
                .replace("\r\n", "\n"),
            "second proof\n"
        );
        fs::write(
            repo.join("feature.txt"),
            "base\nworker change\nmodification utilisateur\n",
        )
        .unwrap();
        assert!(apply_final_patch(&run).is_err());

        remove_owned_worktrees(&run, &sandboxes).unwrap();
        run_git(&repo, ["reset", "--hard", &base], None).unwrap();
        run_git(&repo, ["clean", "-fd"], None).unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cleanup_refuses_a_sandbox_outside_the_trusted_root() {
        let root = std::env::temp_dir().join(format!("cst-orchestration-test-{}", Uuid::new_v4()));
        let trusted = root.join("trusted");
        let outside = root.join("outside");
        fs::create_dir_all(&trusted).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep\n").unwrap();

        let mut run = sample_run();
        run.id = Uuid::new_v4().to_string();
        run.sandbox_root = outside.to_string_lossy().to_string();
        run.orchestrator_dir = outside.join("orchestrator").to_string_lossy().to_string();
        run.tasks.clear();

        assert!(remove_owned_worktrees(&run, &trusted).is_err());
        assert!(outside.join("keep.txt").is_file());
        fs::remove_dir_all(root).unwrap();
    }

    fn sample_run() -> OrchestrationSnapshot {
        OrchestrationSnapshot {
            id: "run-1".to_string(),
            name: "Feature".to_string(),
            objective: "Construire".to_string(),
            worker_count: 1,
            tester_count: 1,
            adaptive_fanout: true,
            max_task_count: DEFAULT_MAX_TASK_COUNT,
            minimum_task_count: 1,
            max_concurrency: DEFAULT_MAX_CONCURRENCY,
            account_id: "account-1".to_string(),
            orchestrator_account_id: "account-1".to_string(),
            worker_account_ids: vec!["account-1".to_string()],
            orchestrator_handoff_pending: false,
            orchestrator_handoff_count: 0,
            owner_id: None,
            source_kind: OrchestrationSourceKind::GitClean,
            requested_project_dir: Some("/repo".to_string()),
            access_project_dir: Some("/repo".to_string()),
            project_dir: "/repo".to_string(),
            model: None,
            reasoning_effort: None,
            test_command: "cargo test".to_string(),
            test_timeout_seconds: 60,
            status: OrchestrationStatus::Active,
            phase: OrchestrationPhase::FinalReview,
            created_at: 1,
            updated_at: 1,
            base_commit: "base".to_string(),
            integrated_commit: "head".to_string(),
            sandbox_root: "/tmp/run".to_string(),
            orchestrator_dir: "/tmp/run/orchestrator".to_string(),
            orchestrator_session_id: None,
            current_turn_id: None,
            current_turn_kind: None,
            current_task_id: None,
            current_tester_id: None,
            current_start_id: None,
            current_validation_id: None,
            current_validation_kind: None,
            next_action_at: None,
            plan_summary: None,
            tasks: vec![OrchestrationTask {
                id: "task-01".to_string(),
                position: 1,
                title: "API".to_string(),
                description: "Ajouter l'API".to_string(),
                acceptance_criteria: vec!["Test".to_string()],
                status: OrchestrationTaskStatus::Accepted,
                account_id: "account-1".to_string(),
                handoff_pending: false,
                handoff_count: 0,
                session_id: None,
                workspace_dir: None,
                base_commit: None,
                workspace_generation: 0,
                attempt_count: 1,
                protocol_failures: 0,
                evidence: None,
                reviews: Vec::new(),
                last_error: None,
            }],
            testers: vec![OrchestrationTester {
                id: "tester-01".to_string(),
                position: 1,
                status: OrchestrationTesterStatus::Passed,
                assigned_task_ids: vec!["task-01".to_string()],
                session_id: None,
                plan_summary: Some("Valider l'API".to_string()),
                test_plan: vec![OrchestrationTestDefinition {
                    name: "Suite Rust".to_string(),
                    command: "cargo test".to_string(),
                    expected: "Tous les tests passent".to_string(),
                }],
                last_results: vec![OrchestrationProofTest {
                    command: "cargo test".to_string(),
                    result: "ok".to_string(),
                    passed: true,
                }],
                attempt_count: 1,
                protocol_failures: 0,
                last_error: None,
            }],
            merge_reviewed_task_ids: Vec::new(),
            final_summary: None,
            last_error: None,
            consecutive_start_failures: 0,
            protocol_failures: 0,
            publish_applied: false,
            team_messages: Vec::new(),
            events: Vec::new(),
        }
    }
}
