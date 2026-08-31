import { randomUUID } from "node:crypto";

import { canonicalJson, sha256 } from "./canonical.mjs";
import {
  MAX_LOGICAL_AGENTS,
  RUN_SPEC_SCHEMA_VERSION,
  createAgentPlan,
} from "./factory.mjs";
import { normalizeFakeScenario } from "./fake-provider.mjs";

const SHA_OR_COMMIT = /^[a-f0-9]{7,64}$/u;
const NAME_MAX = 120;
const PROJECT_MAX = 4_096;
export const SWITCH_READONLY_EXECUTOR = "switch_readonly";
export const SWITCH_READONLY_APPROVAL = "switch-development-readonly/v1";

const integer = (value, label, minimum, maximum) => {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new TypeError(`${label} must be between ${minimum} and ${maximum}`);
  }
  return parsed;
};

const text = (value, label, maximum) => {
  if (typeof value !== "string" || value.trim().length === 0 || value.length > maximum) {
    throw new TypeError(`${label} is required and must not exceed ${maximum} characters`);
  }
  return value.trim();
};

const runId = () => `run_${randomUUID().replaceAll("-", "")}`;

const summarize = (run) => ({
  id: run.id,
  name: run.name,
  status: run.status,
  phase: run.phase,
  createdAt: run.createdAt,
  updatedAt: run.updatedAt,
  logicalAgentCount: run.logicalAgentCount,
  counters: run.counters,
  executor: run.executor,
  planSha256: run.planSha256,
  instancePlanSha256: run.instancePlanSha256,
  configSha256: run.configSha256,
  instanceConfigSha256: run.instanceConfigSha256,
  replaySha256: run.replaySha256 ?? null,
});

const runtimeAgent = (agent) => ({
  id: agent.id,
  logicalKey: agent.logicalKey,
  idempotencyKey: agent.idempotencyKey,
  ordinal: agent.ordinal,
  parentId: agent.parentId,
  batchId: agent.batchId,
  roleId: agent.roleId,
  roleVersion: agent.roleVersion,
  roleIdentitySha256: agent.roleIdentitySha256,
  archetypeId: agent.archetypeId,
  archetype: agent.archetype,
  executionKind: agent.executionKind,
  writeAccess: agent.writeAccess,
  baseCommit: agent.baseCommit,
  scope: agent.scope,
  contextRef: `${agent.batchId}:${agent.contextSha256}`,
  contextBytes: agent.contextBytes,
  contextSha256: agent.contextSha256,
  capsule: agent.capsule,
  capsuleSha256: agent.capsuleSha256,
  outputSchema: agent.outputSchema,
  outputSchemaSha256: agent.outputSchemaSha256,
  sourceChatKey: `mass-subagents:${sha256(agent.idempotencyKey).slice(0, 48)}`,
  status: "queued",
  attemptCount: 0,
  availableAt: null,
  outputSha256: null,
  outputArtifactSha256: null,
  waveArtifactSha256: null,
  verificationStage: null,
  dispatchToken: null,
  dispatchStartedAt: null,
  switchTurnId: null,
  switchStatus: null,
  lastPolledAt: null,
  submittedAt: null,
  attentionReason: null,
  pollErrorCount: 0,
});

export class RealDispatchGateError extends Error {
  constructor(message, code = "REAL_DISPATCH_DISABLED") {
    super(message);
    this.name = "RealDispatchGateError";
    this.code = code;
  }
}

export class MassSubagentService {
  constructor({
    store,
    scheduler,
    registry,
    switchClient = null,
    realDispatchEnabled = false,
    maxRealAgents = 32,
    maxStoredRuns = 100,
    clock = Date.now,
  }) {
    if (!store || !scheduler || !registry) {
      throw new TypeError("store, scheduler, and registry are required");
    }
    this.store = store;
    this.scheduler = scheduler;
    this.registry = registry;
    this.switchClient = switchClient;
    this.realDispatchEnabled = realDispatchEnabled === true;
    this.maxRealAgents = integer(maxRealAgents, "maxRealAgents", 1, MAX_LOGICAL_AGENTS);
    this.maxStoredRuns = integer(maxStoredRuns, "maxStoredRuns", 1, 100_000);
    this.clock = clock;
    this.creationQueue = Promise.resolve();
  }

  roles() {
    return {
      schemaVersion: this.registry.schema_version,
      registrySha256: this.registry.registry_sha256,
      roles: Object.values(this.registry.roles).map((role) => ({
        roleId: role.role_id,
        roleVersion: role.role_version,
        identitySha256: role.identity_sha256,
        executionKind: role.execution_kind,
        archetypeId: role.archetype_id,
        archetype: role.archetype,
        writeAccess: role.write_access,
        model: role.model,
        reasoning: role.reasoning,
        serviceTier: role.service_tier,
        toolsAllowlist: role.tools_allowlist,
        outputContract: role.output_contract,
        requireReview: role.require_review,
      })),
    };
  }

  create(candidate = {}) {
    const operation = this.creationQueue.then(() => this.createSerialized(candidate));
    this.creationQueue = operation.catch(() => {});
    return operation;
  }

  async hasActiveReadonlyRun() {
    if (typeof this.store.list !== "function") return false;
    let cursor = 0;
    do {
      const page = await this.store.list({ limit: 500, cursor });
      if (page.items.some((run) => run.executor?.kind === SWITCH_READONLY_EXECUTOR
        && ["active", "paused", "cancelling"].includes(run.status))) return true;
      cursor = page.nextCursor;
    } while (cursor !== null);
    return false;
  }

  async findReadonlyRunByClientRequestId(clientRequestId) {
    if (typeof this.store.list !== "function" || typeof this.store.get !== "function") return null;
    let cursor = 0;
    do {
      const page = await this.store.list({ limit: 500, cursor });
      for (const item of page.items) {
        if (item.executor?.kind !== SWITCH_READONLY_EXECUTOR) continue;
        const run = await this.store.get(item.id);
        if (run.definition?.clientRequestId === clientRequestId) return run;
      }
      cursor = page.nextCursor;
    } while (cursor !== null);
    return null;
  }

  async createSerialized(candidate = {}) {
    if (!candidate || typeof candidate !== "object" || Array.isArray(candidate)) {
      throw new TypeError("run request must be an object");
    }
    const id = runId();
    const name = text(candidate.name ?? "Mass subagents run", "name", NAME_MAX);
    const projectDir = text(candidate.projectDir, "projectDir", PROJECT_MAX);
    const executorKind = candidate.executor ?? "fake";
    if (!["fake", SWITCH_READONLY_EXECUTOR].includes(executorKind)) {
      throw new TypeError("executor must be fake or switch_readonly");
    }
    const capacity = integer(
      candidate.capacity ?? this.scheduler.capacity,
      "capacity",
      1,
      this.scheduler.capacity,
    );
    const microtasksPerSession = integer(
      candidate.microtasksPerSession ?? this.scheduler.microtasksPerSession,
      "microtasksPerSession",
      1,
      256,
    );
    const fakeScenario = normalizeFakeScenario(
      executorKind === "fake" ? candidate.fakeScenario : undefined,
    );
    const requested = candidate.specification;
    if (!requested || typeof requested !== "object" || Array.isArray(requested)) {
      throw new TypeError("specification is required");
    }
    if (!SHA_OR_COMMIT.test(requested.baseCommit ?? "")) {
      throw new TypeError("specification.baseCommit must be an explicit Git commit");
    }
    const specification = {
      ...requested,
      schemaVersion: RUN_SPEC_SCHEMA_VERSION,
      runId: id,
    };
    const plan = await createAgentPlan(specification, { registry: this.registry });
    if (plan.logicalAgentCount < 1 || plan.logicalAgentCount > MAX_LOGICAL_AGENTS) {
      throw new Error("factory returned an invalid logical-agent count");
    }
    let accountId = null;
    let maxChatTurns = null;
    let clientRequestId = null;
    let clientRequestSha256 = null;
    if (executorKind === SWITCH_READONLY_EXECUTOR) {
      if (!this.realDispatchEnabled
        || !this.switchClient?.authenticated
        || !this.switchClient?.chatDispatchEnabled) {
        throw new RealDispatchGateError(
          "Switch read-only chat dispatch is not enabled in this development sidecar.",
        );
      }
      if (candidate.realDispatchApproval !== SWITCH_READONLY_APPROVAL) {
        throw new RealDispatchGateError(
          `realDispatchApproval must equal ${SWITCH_READONLY_APPROVAL}.`,
          "REAL_DISPATCH_APPROVAL_REQUIRED",
        );
      }
      if (plan.logicalAgentCount > this.maxRealAgents) {
        throw new RealDispatchGateError(
          `real dispatch is limited to ${this.maxRealAgents} agents by this deployment.`,
          "REAL_DISPATCH_BUDGET_EXCEEDED",
        );
      }
      maxChatTurns = integer(
        candidate.maxChatTurns,
        "maxChatTurns",
        1,
        this.maxRealAgents,
      );
      if (maxChatTurns !== plan.logicalAgentCount) {
        throw new RealDispatchGateError(
          "maxChatTurns must exactly equal the expanded logical-agent count.",
          "REAL_DISPATCH_BUDGET_MISMATCH",
        );
      }
      accountId = text(candidate.accountId, "accountId", 256);
      clientRequestId = text(candidate.idempotencyKey, "idempotencyKey", 128);
      if (!/^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u.test(clientRequestId)) {
        throw new TypeError("idempotencyKey has an invalid format");
      }
      clientRequestSha256 = sha256(candidate);
      const unsafe = plan.agents.filter((agent) =>
        agent.executionKind !== "chat" || agent.writeAccess !== "read");
      if (unsafe.length > 0) {
        throw new RealDispatchGateError(
          `switch_readonly refuses non-read roles: ${[...new Set(unsafe.map(({ roleId }) => roleId))].join(", ")}.`,
          "REAL_DISPATCH_ROLE_REFUSED",
        );
      }
      const previous = await this.findReadonlyRunByClientRequestId(clientRequestId);
      if (previous) {
        if (previous.definition?.clientRequestSha256 !== clientRequestSha256) {
          throw new RealDispatchGateError(
            "idempotencyKey was already used with a different real run request.",
            "REAL_DISPATCH_IDEMPOTENCY_CONFLICT",
          );
        }
        return { ...summarize(previous), idempotentReplay: true };
      }
      if (await this.hasActiveReadonlyRun()) {
        throw new RealDispatchGateError(
          "only one live switch_readonly run is allowed by the pilot scheduler.",
          "REAL_DISPATCH_RUN_CONFLICT",
        );
      }
    }
    if (typeof this.store.list === "function") {
      const page = await this.store.list({ limit: 1, cursor: 0 });
      if (page.total >= this.maxStoredRuns) {
        const error = new Error(
          `run retention limit ${this.maxStoredRuns} reached; archive data before creating a run`,
        );
        error.code = "STORE_RUN_LIMIT_REACHED";
        throw error;
      }
    }
    const now = new Date(this.clock()).toISOString();
    const usedRoleIds = new Set(plan.agents.map(({ roleId }) => roleId));
    const roleMaterials = Object.fromEntries([...usedRoleIds].sort().map((roleId) => {
      const role = this.registry.roles[roleId];
      return [roleId, {
        roleVersion: role.role_version,
        identitySha256: role.identity_sha256,
        capsuleSha256: role.capsule_sha256,
        capsuleSource: role.capsule_source,
        outputSchemaSha256: role.output_schema_sha256,
        outputSchema: role.output_schema,
        model: role.model,
        reasoning: role.reasoning,
      }];
    }));
    const contextCatalog = Object.fromEntries(plan.agents.map((agent) => [
      agent.contextSha256,
      agent.context,
    ]));
    const definition = {
      schemaVersion: "switch-mass-subagents/run-definition/v1",
      name,
      projectDir,
      seed: specification.seed,
      baseCommit: specification.baseCommit,
      fakeScenario,
      accountId,
      maxChatTurns,
      clientRequestId,
      clientRequestSha256,
      contextCatalog,
      roleMaterials,
      specification,
      roleRegistrySha256: this.registry.registry_sha256,
      factoryPlanSha256: plan.planSha256,
      factoryInstancePlanSha256: plan.instancePlanSha256,
      realChatDispatchEnabled: executorKind === SWITCH_READONLY_EXECUTOR,
    };
    const { runId: _runId, ...replaySpecification } = specification;
    const replayDefinition = {
      ...definition,
      specification: replaySpecification,
      factoryInstancePlanSha256: undefined,
    };
    const record = {
      id,
      name,
      projectDir,
      createdAt: now,
      updatedAt: now,
      status: candidate.startPaused === true ? "paused" : "active",
      phase: executorKind === "fake" ? "phase_a_queued" : "switch_readonly_queued",
      logicalAgentCount: plan.logicalAgentCount,
      configSha256: sha256(replayDefinition),
      instanceConfigSha256: sha256(definition),
      planSha256: plan.planSha256,
      instancePlanSha256: plan.instancePlanSha256,
      definition,
      executor: {
        kind: executorKind,
        capacity,
        microtasksPerSession,
        maxChatTurns,
        mode: executorKind === SWITCH_READONLY_EXECUTOR ? "ask" : null,
        toolScope: executorKind === SWITCH_READONLY_EXECUTOR ? "none" : null,
        realChatDispatchEnabled: executorKind === SWITCH_READONLY_EXECUTOR,
      },
      batches: plan.batches.map((batch) => ({
        id: batch.id,
        dependsOn: batch.dependsOn,
        order: batch.order,
        wave: batch.wave,
        parentId: batch.parentId,
        agentCount: batch.agentIds.length,
      })),
      agents: plan.agents.map(runtimeAgent),
      counters: {
        queued: plan.logicalAgentCount,
        retryWait: 0,
        dispatching: 0,
        running: 0,
        submitted: 0,
        needsAttention: 0,
        verified: 0,
        failed: 0,
        cancelled: 0,
        retries: 0,
        duplicatesIgnored: 0,
        sessionWaves: 0,
        sessionsUsed: 0,
        peakSessions: 0,
      },
      nextEventSequence: 2,
    };
    await this.store.create(record, [{
      sequence: 1,
      timestamp: now,
      kind: "run_created",
      configSha256: record.configSha256,
      planSha256: plan.planSha256,
      logicalAgentCount: plan.logicalAgentCount,
    }]);
    return summarize(record);
  }

  async list(options) {
    const page = await this.store.list(options);
    return { ...page, items: page.items.map(summarize) };
  }

  async get(id) {
    const run = await this.store.get(id);
    return {
      ...summarize(run),
      projectDir: run.projectDir,
      definitionSha256: sha256(run.definition),
      batches: run.batches,
    };
  }

  agents(id, options) {
    return this.store.pageAgents(id, options);
  }

  control(id, action) {
    return this.scheduler.control(id, action).then(summarize);
  }

  async probeSwitchContract(options) {
    if (!this.switchClient) throw new Error("Switch development client is not configured");
    const result = await this.switchClient.probeContract(options);
    const safe = {
      health: result.health,
      activeChatTurnCount: result.activeChatTurns.length,
      orchestrationCount: result.orchestrations.length,
      probedAt: new Date(this.clock()).toISOString(),
    };
    return { ...safe, evidenceSha256: sha256(safe) };
  }
}

export const serviceInternals = Object.freeze({ runtimeAgent, summarize });
