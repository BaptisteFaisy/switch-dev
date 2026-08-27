export type OrchestrationStatus = "active" | "paused" | "completed" | "needs_attention";
export type OrchestrationSourceKind = "git_clean" | "git_dirty" | "ephemeral";

export type OrchestrationPhase =
  | "planning"
  | "working"
  | "reviewing"
  | "validating"
  | "final_review"
  | "final_validation"
  | "publishing"
  | "completed";

export type OrchestrationTaskStatus =
  | "pending"
  | "working"
  | "submitted"
  | "reviewing"
  | "validating"
  | "revision_requested"
  | "accepted";

export type OrchestrationProofTest = {
  command: string;
  result: string;
  passed: boolean;
};

export type OrchestrationProof = {
  summary: string;
  filesChanged: string[];
  tests: OrchestrationProofTest[];
  risks: string[];
  submittedAt: number;
};

export type OrchestrationReview = {
  decision: "accept" | "revise";
  summary: string;
  feedback: string;
  tests: OrchestrationProofTest[];
  createdAt: number;
};

export type OrchestrationTask = {
  id: string;
  position: number;
  title: string;
  description: string;
  acceptanceCriteria: string[];
  status: OrchestrationTaskStatus;
  /** Compte affecte a ce worker. Absent uniquement sur les anciens snapshots. */
  accountId?: string;
  /** Une reprise par transcript sera injectee au prochain tour de ce worker. */
  handoffPending?: boolean;
  handoffCount?: number;
  sessionId: string | null;
  workspaceDir: string | null;
  baseCommit: string | null;
  workspaceGeneration: number;
  attemptCount: number;
  protocolFailures: number;
  evidence: OrchestrationProof | null;
  reviews: OrchestrationReview[];
  lastError: string | null;
};

export type OrchestrationEvent = {
  timestamp: number;
  kind: string;
  message: string;
};

export type OrchestrationSnapshot = {
  id: string;
  name: string;
  objective: string;
  /** Nombre de travailleurs choisis, sans compter l'orchestrateur. */
  workerCount: number;
  adaptiveFanout?: boolean;
  maxTaskCount?: number;
  minimumTaskCount?: number;
  maxConcurrency?: number;
  /** `accountId` reste le repli des snapshots crees avant les affectations par role. */
  accountId: string;
  orchestratorAccountId?: string;
  workerAccountIds?: string[];
  orchestratorHandoffPending?: boolean;
  orchestratorHandoffCount?: number;
  ownerId?: string | null;
  sourceKind?: OrchestrationSourceKind;
  requestedProjectDir?: string | null;
  accessProjectDir?: string | null;
  projectDir: string;
  model: string | null;
  reasoningEffort: string | null;
  testCommand: string;
  testTimeoutSeconds: number;
  status: OrchestrationStatus;
  phase: OrchestrationPhase;
  createdAt: number;
  updatedAt: number;
  baseCommit: string;
  integratedCommit: string;
  sandboxRoot: string;
  orchestratorDir: string;
  orchestratorSessionId: string | null;
  currentTurnId: number | null;
  currentTurnKind: "plan" | "worker" | "review" | "final_review" | null;
  currentTaskId: string | null;
  currentStartId: string | null;
  currentValidationId: string | null;
  currentValidationKind: "task" | "final" | null;
  nextActionAt: number | null;
  planSummary: string | null;
  tasks: OrchestrationTask[];
  finalSummary: string | null;
  lastError: string | null;
  consecutiveStartFailures: number;
  protocolFailures: number;
  publishApplied: boolean;
  events: OrchestrationEvent[];
};

export type OrchestrationAction = "pause" | "resume" | "retry";

export type OrchestrationAccountRole = "orchestrator" | "worker";

export const AUTOMATIC_ORCHESTRATION_MARKER = "CST_AUTO_ORCHESTRATION:";

export const DEFAULT_ORCHESTRATION_WORKER_COUNT = 3;

/** Budget maximal d'agents autorisé à coder sur un même travail. */
export const MAX_ORCHESTRATION_WORKER_COUNT = 200;

export const normalizeOrchestrationWorkerCount = (value: number): number | null => {
  if (!Number.isInteger(value) || value < 1 || value > MAX_ORCHESTRATION_WORKER_COUNT) return null;
  if (value <= 4) return value;
  if (value <= 20) return 20;
  if (value <= 100) return 100;
  return 200;
};

export const orchestrationWorkerCountFromEnv = (value: unknown): number => {
  if (typeof value !== "string") return DEFAULT_ORCHESTRATION_WORKER_COUNT;
  const normalized = value.trim();
  if (!/^\d+$/.test(normalized)) return DEFAULT_ORCHESTRATION_WORKER_COUNT;
  return normalizeOrchestrationWorkerCount(Number(normalized))
    ?? DEFAULT_ORCHESTRATION_WORKER_COUNT;
};

export type AutomaticOrchestrationDecision = {
  workerCount: number;
  reason: string;
};

export type DefaultOrchestrationSurface = "classic-chat" | "freebuff-terminal";

export type DefaultOrchestrationContext = {
  surface: DefaultOrchestrationSurface;
  orchestrationRole?: "orchestrator" | "worker" | null;
  autonomousAgentId?: string | null;
  /** Ancienne preference conservee uniquement pour tester/migrer les sessions. */
  persistedEnabled?: boolean;
};

/**
 * Tous les points d'entree utilisateur sont orchestrateurs par defaut.
 *
 * Un chat deja gere par une orchestration (orchestrateur ou worker) et un
 * agent autonome restent exclus afin de ne jamais creer une equipe recursive.
 * Freebuff n'expose qu'un terminal racine : son propre orchestrateur natif est
 * donc toujours actif, meme si une ancienne preference de chat valait `false`.
 */
export const automaticOrchestrationEnabledByDefault = (
  context: DefaultOrchestrationContext,
): boolean =>
  context.surface === "freebuff-terminal"
  || (!context.orchestrationRole && !context.autonomousAgentId);

/**
 * Lit uniquement la ligne de routage emise par le modele. Une mention du
 * protocole dans une explication ordinaire ne doit jamais lancer une equipe.
 */
export const parseAutomaticOrchestrationDecision = (
  text: string | null | undefined,
): AutomaticOrchestrationDecision | null => {
  if (!text) return null;
  const line = text
    .split(/\r?\n/)
    .map((candidate) => candidate.trim())
    .reverse()
    .find((candidate) => candidate.startsWith(AUTOMATIC_ORCHESTRATION_MARKER));
  if (!line) return null;

  try {
    const value = JSON.parse(line.slice(AUTOMATIC_ORCHESTRATION_MARKER.length).trim()) as {
      decision?: unknown;
      workerCount?: unknown;
      reason?: unknown;
    };
    if (value?.decision !== "orchestrate") return null;
    const workerCount = normalizeOrchestrationWorkerCount(Number(value.workerCount));
    if (workerCount === null) return null;
    return {
      workerCount,
      reason: typeof value.reason === "string" ? value.reason.trim().slice(0, 500) : "",
    };
  } catch {
    return null;
  }
};

export const orchestrationRequestedProjectDir = (
  run: Pick<OrchestrationSnapshot, "projectDir" | "requestedProjectDir">,
): string => run.requestedProjectDir?.trim() || run.projectDir;

export const orchestrationMinimumTaskCount = (
  run: Pick<OrchestrationSnapshot, "workerCount" | "minimumTaskCount">,
): number => Math.max(1, run.minimumTaskCount ?? run.workerCount);

export const automaticOrchestrationNotice = (
  decision: AutomaticOrchestrationDecision,
): string =>
  `Orchestration automatique retenue · ${decision.workerCount} worker${decision.workerCount > 1 ? "s" : ""}${decision.reason ? ` · ${decision.reason}` : ""}`;

export const orchestrationOrchestratorAccountId = (
  run: Pick<OrchestrationSnapshot, "accountId" | "orchestratorAccountId">,
): string => run.orchestratorAccountId?.trim() || run.accountId;

export const orchestrationWorkerAccountId = (
  run: Pick<OrchestrationSnapshot, "accountId" | "workerAccountIds">,
  task: Pick<OrchestrationTask, "position" | "accountId">,
): string =>
  task.accountId?.trim()
  || run.workerAccountIds?.[Math.max(0, task.position - 1)]?.trim()
  || run.accountId;

export const orchestrationStatusLabel = (status: OrchestrationStatus): string => {
  switch (status) {
    case "active":
      return "En cours";
    case "paused":
      return "En pause";
    case "completed":
      return "Rendu";
    case "needs_attention":
      return "Attention requise";
  }
};

export const orchestrationPhaseLabel = (phase: OrchestrationPhase): string => {
  switch (phase) {
    case "planning":
      return "Planification par l’orchestrateur";
    case "working":
      return "Travail des agents";
    case "reviewing":
      return "Revue dans le sandbox orchestrateur";
    case "validating":
      return "Validation réelle de la contribution";
    case "final_review":
      return "Audit final";
    case "final_validation":
      return "Validation finale";
    case "publishing":
      return "Application du rendu";
    case "completed":
      return "Projet rendu";
  }
};

export const orchestrationTaskStatusLabel = (status: OrchestrationTaskStatus): string => {
  switch (status) {
    case "pending":
      return "Chat en attente";
    case "working":
      return "Agent au travail";
    case "submitted":
      return "Preuve soumise";
    case "reviewing":
      return "Revue orchestrateur";
    case "validating":
      return "Tests réels";
    case "revision_requested":
      return "Correction demandée";
    case "accepted":
      return "Acceptée et intégrée";
  }
};

export const orchestrationProgress = (
  run: Pick<OrchestrationSnapshot, "tasks" | "phase">,
): { accepted: number; total: number; percent: number } => {
  const total = run.tasks.length;
  const accepted = run.tasks.filter((task) => task.status === "accepted").length;
  if (!total) return { accepted, total, percent: run.phase === "planning" ? 0 : 100 };
  return { accepted, total, percent: Math.round((accepted / total) * 100) };
};

export const orchestrationIsRunning = (
  run: Pick<
    OrchestrationSnapshot,
    "status" | "currentTurnId" | "currentStartId" | "currentValidationId"
  >,
): boolean =>
  run.status === "active"
  && (run.currentTurnId != null || run.currentStartId != null || run.currentValidationId != null);
