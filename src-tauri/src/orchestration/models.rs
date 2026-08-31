use super::{default_adaptive_fanout, default_max_concurrency, default_max_task_count};
use serde::{Deserialize, Serialize};

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
