import { randomUUID } from "node:crypto";

import { DeterministicFakeProvider } from "./fake-provider.mjs";
import { sha256 } from "./canonical.mjs";
import { SwitchReadonlyExecutor } from "./switch-readonly-executor.mjs";

const TERMINAL_AGENT_STATUSES = new Set([
  "verified",
  "submitted",
  "failed",
  "cancelled",
  "needs_attention",
]);
const TERMINAL_RUN_STATUSES = new Set(["completed", "cancelled", "needs_attention"]);
const RUNNABLE_AGENT_STATUSES = new Set(["queued", "retry_wait"]);
const DEPENDENCY_SUCCESS_STATUSES = new Set(["verified", "submitted"]);

const boundedInteger = (value, label, minimum, maximum) => {
  const parsed = Number(value);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new TypeError(`${label} must be between ${minimum} and ${maximum}`);
  }
  return parsed;
};

const nowIso = (clock) => new Date(clock()).toISOString();

const nextEvent = (run, clock, kind, details = {}) => ({
  sequence: run.nextEventSequence++,
  timestamp: nowIso(clock),
  kind,
  ...details,
});

const batchIsReady = (run, batchId) => {
  const batch = run.batches.find((candidate) => candidate.id === batchId);
  if (!batch) return false;
  return batch.dependsOn.every((dependencyId) => run.agents
    .filter((agent) => agent.batchId === dependencyId)
    .every((agent) => DEPENDENCY_SUCCESS_STATUSES.has(agent.status)));
};

const propagateDependencyFailures = (run) => {
  const blockedBatchIds = new Set();
  let blockedAgentCount = 0;
  let changed;
  do {
    changed = false;
    for (const batch of run.batches) {
      const blocked = batch.dependsOn.some((dependencyId) => run.agents
        .filter((agent) => agent.batchId === dependencyId)
        .some((agent) => ["failed", "cancelled", "needs_attention"].includes(agent.status)));
      if (!blocked) continue;
      for (const agent of run.agents) {
        if (agent.batchId !== batch.id || TERMINAL_AGENT_STATUSES.has(agent.status)) continue;
        agent.status = "failed";
        agent.availableAt = null;
        agent.failureReason = "dependency_failed";
        blockedBatchIds.add(batch.id);
        blockedAgentCount += 1;
        changed = true;
      }
    }
  } while (changed);
  return {
    blockedAgentCount,
    blockedBatchIds: [...blockedBatchIds].sort(),
  };
};

const recomputeCounters = (run) => {
  const counters = {
    queued: 0,
    retryWait: 0,
    dispatching: 0,
    running: 0,
    submitted: 0,
    needsAttention: 0,
    verified: 0,
    failed: 0,
    cancelled: 0,
  };
  for (const agent of run.agents) {
    if (agent.status === "queued") counters.queued += 1;
    else if (agent.status === "retry_wait") counters.retryWait += 1;
    else if (agent.status === "dispatching") counters.dispatching += 1;
    else if (agent.status === "running") counters.running += 1;
    else if (agent.status === "submitted") counters.submitted += 1;
    else if (agent.status === "needs_attention") counters.needsAttention += 1;
    else if (agent.status === "verified") counters.verified += 1;
    else if (agent.status === "failed") counters.failed += 1;
    else if (agent.status === "cancelled") counters.cancelled += 1;
  }
  Object.assign(run.counters, counters);
};

const logicalReplayHash = (run) => sha256({
  seed: run.definition.seed,
  configSha256: run.configSha256,
  agents: run.agents.map((agent) => ({
    logicalKey: agent.logicalKey,
    status: agent.status,
    attemptCount: agent.attemptCount,
    outputSha256: agent.outputSha256 ?? null,
  })),
  counters: {
    verified: run.counters.verified,
    submitted: run.counters.submitted ?? 0,
    needsAttention: run.counters.needsAttention ?? 0,
    failed: run.counters.failed,
    cancelled: run.counters.cancelled,
    retries: run.counters.retries,
    duplicatesIgnored: run.counters.duplicatesIgnored,
  },
});

export class MassSubagentScheduler {
  constructor({
    store,
    capacity,
    microtasksPerSession = 32,
    maxAttempts = 3,
    maxPollErrors = 5,
    intervalMs = 25,
    switchClient = null,
    dispatchRecoveryMs = 30_000,
    clock = Date.now,
  }) {
    if (!store) throw new TypeError("store is required");
    this.store = store;
    this.capacity = boundedInteger(capacity, "capacity", 1, 1_024);
    this.microtasksPerSession = boundedInteger(
      microtasksPerSession,
      "microtasksPerSession",
      1,
      256,
    );
    this.maxAttempts = boundedInteger(maxAttempts, "maxAttempts", 1, 20);
    this.maxPollErrors = boundedInteger(maxPollErrors, "maxPollErrors", 1, 100);
    this.intervalMs = boundedInteger(intervalMs, "intervalMs", 5, 60_000);
    this.dispatchRecoveryMs = boundedInteger(
      dispatchRecoveryMs,
      "dispatchRecoveryMs",
      1_000,
      600_000,
    );
    this.readonlyExecutor = switchClient?.chatDispatchEnabled
      ? new SwitchReadonlyExecutor({ switchClient, store })
      : null;
    this.clock = clock;
    this.timer = null;
    this.ticking = false;
    this.idleWaiters = new Set();
  }

  start() {
    if (this.timer) return;
    this.timer = setInterval(() => {
      this.tick().catch((error) => {
        process.stderr.write(`[mass-subagents] scheduler tick failed: ${error.message}\n`);
      });
    }, this.intervalMs);
    this.timer.unref?.();
  }

  stop() {
    if (this.timer) clearInterval(this.timer);
    this.timer = null;
    if (!this.ticking) return Promise.resolve();
    return new Promise((resolve) => this.idleWaiters.add(resolve));
  }

  async tick() {
    if (this.ticking) return [];
    this.ticking = true;
    try {
      const outcomes = [];
      let cursor = 0;
      do {
        const page = await this.store.list({ limit: 500, cursor });
        for (const run of page.items) {
          if (run.status === "active" && run.executor?.kind === "fake") {
            outcomes.push(await this.tickRun(run.id));
          } else if (["active", "paused"].includes(run.status)
            && run.executor?.kind === "switch_readonly") {
            outcomes.push(await this.tickSwitchRun(run.id));
          }
        }
        cursor = page.nextCursor;
      } while (cursor !== null);
      return outcomes;
    } finally {
      this.ticking = false;
      const waiters = [...this.idleWaiters];
      this.idleWaiters.clear();
      for (const resolve of waiters) resolve();
    }
  }

  async tickRun(runId) {
    const initial = await this.store.get(runId);
    if (initial.status !== "active" || initial.executor?.kind !== "fake") {
      return { runId, processed: 0 };
    }
    const now = this.clock();
    const events = [];
    let outcome = { runId, processed: 0 };
    const updated = await this.store.update(runId, async (run) => {
      if (run.status !== "active" || run.executor?.kind !== "fake") return run;
      const runCapacity = boundedInteger(
        run.executor.capacity ?? this.capacity,
        "run executor capacity",
        1,
        this.capacity,
      );
      const runMicrotasksPerSession = boundedInteger(
        run.executor.microtasksPerSession ?? this.microtasksPerSession,
        "run microtasksPerSession",
        1,
        256,
      );
      const limit = runCapacity * runMicrotasksPerSession;
      const selected = run.agents
        .filter((agent) => RUNNABLE_AGENT_STATUSES.has(agent.status))
        .filter((agent) => !agent.availableAt || Date.parse(agent.availableAt) <= now)
        .filter((agent) => batchIsReady(run, agent.batchId))
        .slice(0, limit);
      if (selected.length === 0) return run;

      const provider = new DeterministicFakeProvider({
        seed: run.definition.seed,
        scenario: run.definition.fakeScenario,
      });
      const executions = selected.map((agent) => {
        const attempt = agent.attemptCount + 1;
        return {
          agentId: agent.id,
          logicalKey: agent.logicalKey,
          previousAttemptCount: agent.attemptCount,
          attempt,
          outcome: provider.execute({
            logicalKey: agent.logicalKey,
            ordinal: agent.ordinal,
            attempt,
            idempotencyKey: agent.idempotencyKey,
          }),
        };
      });
      let results = executions.flatMap((result) => result.outcome.duplicateDelivery
        ? [result, { ...result, duplicateReplay: true }]
        : [result]);
      results.sort((left, right) =>
        (left.outcome.latencyMs ?? 0) - (right.outcome.latencyMs ?? 0)
        || left.logicalKey.localeCompare(right.logicalKey)
        || Number(left.duplicateReplay === true) - Number(right.duplicateReplay === true));
      if (results.some((result) => result.outcome.outOfOrderDelivery)) {
        results = [...results].reverse();
      }
      const sessionCount = Math.ceil(executions.length / runMicrotasksPerSession);
      const artifact = await this.store.putArtifact({
        schema: "switch-mass-subagents/fake-wave/v1",
        runId,
        seed: run.definition.seed,
        sessionCount,
        executionCount: executions.length,
        deliveryCount: results.length,
        results,
      });

      const byId = new Map(run.agents.map((agent) => [agent.id, agent]));
      let applied = 0;
      for (const result of results) {
        const agent = byId.get(result.agentId);
        if (!agent
          || !RUNNABLE_AGENT_STATUSES.has(agent.status)
          || agent.attemptCount !== result.previousAttemptCount) {
          run.counters.duplicatesIgnored += 1;
          continue;
        }
        agent.attemptCount = result.attempt;
        if (result.outcome.status === "retry") {
          run.counters.retries += 1;
          if (agent.attemptCount >= this.maxAttempts) {
            agent.status = "failed";
            agent.failureReason = result.outcome.reason;
          } else {
            agent.status = "retry_wait";
            agent.availableAt = new Date(now + result.outcome.retryAfterMs).toISOString();
          }
        } else {
          agent.status = "verified";
          agent.availableAt = null;
          agent.outputSha256 = result.outcome.payloadSha256;
          agent.waveArtifactSha256 = artifact.sha256;
          agent.verificationStage = "phase_a_fake";
        }
        applied += 1;
      }
      run.counters.sessionWaves += 1;
      run.counters.sessionsUsed += sessionCount;
      run.counters.peakSessions = Math.max(run.counters.peakSessions, sessionCount);
      const blocked = propagateDependencyFailures(run);
      recomputeCounters(run);
      run.updatedAt = nowIso(this.clock);
      run.phase = "dispatching_fake";
      const event = nextEvent(run, this.clock, "fake_wave_committed", {
        applied,
        sessionCount,
        artifactSha256: artifact.sha256,
      });
      events.push(event);
      if (blocked.blockedAgentCount > 0) {
        events.push(nextEvent(run, this.clock, "dependent_batches_blocked", blocked));
      }
      outcome = {
        runId,
        processed: executions.length,
        deliveries: results.length,
        sessionCount,
        artifactSha256: artifact.sha256,
      };
      return run;
    }, events);
    if (updated.agents.every((agent) => TERMINAL_AGENT_STATUSES.has(agent.status))) {
      await this.finishRun(runId);
    }
    return outcome;
  }

  async commitSwitchOutcomes(runId, outcomes, kind) {
    if (outcomes.length === 0) return { runId, applied: 0 };
    const events = [];
    let applied = 0;
    let blocked = { blockedAgentCount: 0, blockedBatchIds: [] };
    const updated = await this.store.update(runId, (run) => {
      for (const entry of outcomes) {
        const agent = run.agents.find(({ id }) => id === entry.agentId);
        if (!agent) continue;
        const dispatchCommit = entry.dispatchToken !== null
          && agent.status === "dispatching"
          && agent.dispatchToken === entry.dispatchToken;
        const pollCommit = entry.switchTurnId !== null
          && agent.status === "running"
          && String(agent.switchTurnId) === String(entry.switchTurnId);
        if (!dispatchCommit && !pollCommit) continue;

        const outcome = entry.outcome;
        agent.lastPolledAt = nowIso(this.clock);
        if (outcome.kind === "retry") {
          agent.dispatchToken = null;
          agent.dispatchStartedAt = null;
          run.counters.retries += 1;
          if (agent.attemptCount >= this.maxAttempts) {
            agent.status = "needs_attention";
            agent.availableAt = null;
            agent.attentionReason = `${outcome.reason ?? "switch_retry"}_retries_exhausted`;
          } else {
            agent.status = "retry_wait";
            agent.availableAt = new Date(this.clock() + this.intervalMs).toISOString();
            agent.attentionReason = outcome.reason ?? "switch_retry";
          }
        } else if (outcome.kind === "not_found") {
          const elapsed = this.clock() - Date.parse(agent.dispatchStartedAt);
          if (Number.isFinite(elapsed) && elapsed < this.dispatchRecoveryMs) continue;
          agent.status = "needs_attention";
          agent.dispatchToken = null;
          agent.attentionReason = "stranded_dispatch_outcome_unknown";
        } else if (outcome.kind === "poll_error") {
          agent.pollErrorCount = (agent.pollErrorCount ?? 0) + 1;
          agent.attentionReason = `transient_${outcome.error.code}`;
          if (agent.pollErrorCount < this.maxPollErrors) continue;
          agent.status = "needs_attention";
          agent.dispatchToken = null;
          agent.attentionReason = `persistent_${outcome.error.code}`;
        } else if (outcome.kind === "running") {
          agent.status = "running";
          agent.switchTurnId = outcome.snapshot.id;
          agent.switchStatus = outcome.snapshot.status;
          agent.dispatchToken = null;
          agent.attentionReason = null;
          agent.pollErrorCount = 0;
        } else if (outcome.kind === "submitted") {
          agent.status = "submitted";
          agent.switchTurnId = outcome.snapshot.id;
          agent.switchStatus = outcome.snapshot.status;
          agent.dispatchToken = null;
          agent.submittedAt = nowIso(this.clock);
          agent.outputSha256 = outcome.outputSha256;
          agent.outputArtifactSha256 = outcome.artifactSha256;
          agent.verificationStage = "role_output_schema";
          agent.attentionReason = null;
          agent.pollErrorCount = 0;
        } else if (["failed", "cancelled"].includes(outcome.kind)) {
          agent.status = outcome.kind;
          agent.switchTurnId = outcome.snapshot.id;
          agent.switchStatus = outcome.snapshot.status;
          agent.dispatchToken = null;
          agent.outputArtifactSha256 = outcome.artifactSha256;
          agent.failureReason = `switch_turn_${outcome.kind}`;
          agent.pollErrorCount = 0;
        } else {
          agent.status = "needs_attention";
          agent.dispatchToken = null;
          agent.attentionReason = outcome.reason ?? "switch_dispatch_attention_required";
          if (outcome.artifactSha256) agent.outputArtifactSha256 = outcome.artifactSha256;
        }
        applied += 1;
      }
      blocked = propagateDependencyFailures(run);
      recomputeCounters(run);
      run.updatedAt = nowIso(this.clock);
      run.phase = "dispatching_switch_readonly";
      events.push(nextEvent(run, this.clock, kind, {
        received: outcomes.length,
        applied,
      }));
      if (blocked.blockedAgentCount > 0) {
        events.push(nextEvent(run, this.clock, "dependent_batches_blocked", blocked));
      }
      return run;
    }, events);
    if (updated.agents.every((agent) => TERMINAL_AGENT_STATUSES.has(agent.status))) {
      await this.finishRun(runId);
    }
    return { runId, applied, blocked: blocked.blockedAgentCount };
  }

  async tickSwitchRun(runId) {
    if (!this.readonlyExecutor) {
      return { runId, processed: 0, disabled: true };
    }
    const initial = await this.store.get(runId);
    if (!["active", "paused"].includes(initial.status)
      || initial.executor?.kind !== "switch_readonly") {
      return { runId, processed: 0 };
    }

    const inFlight = initial.agents.filter((agent) =>
      agent.status === "running" || agent.status === "dispatching");
    const observed = await Promise.all(inFlight.map(async (agent) => ({
      agentId: agent.id,
      dispatchToken: agent.status === "dispatching" ? agent.dispatchToken : null,
      switchTurnId: agent.status === "running" ? agent.switchTurnId : null,
      outcome: agent.status === "running"
        ? await this.readonlyExecutor.poll(initial, agent)
        : await this.readonlyExecutor.reconcile(initial, agent),
    })));
    if (observed.length > 0) {
      await this.commitSwitchOutcomes(runId, observed, "switch_turns_observed");
    }

    const afterPoll = await this.store.get(runId);
    if (afterPoll.status !== "active") {
      return { runId, processed: observed.length, dispatched: 0 };
    }
    const runCapacity = boundedInteger(
      afterPoll.executor.capacity ?? this.capacity,
      "run executor capacity",
      1,
      this.capacity,
    );
    const globalActiveCount = await this.readonlyExecutor.activeTurnCount();
    const slots = Math.max(0, runCapacity - globalActiveCount);
    if (slots === 0) return { runId, processed: observed.length, dispatched: 0 };

    const reservationEvents = [];
    const reservations = [];
    const reserved = await this.store.update(runId, (run) => {
      if (run.status !== "active" || run.executor?.kind !== "switch_readonly") return run;
      const currentlyActive = run.agents.filter((agent) =>
        agent.status === "running" || agent.status === "dispatching").length;
      const availableSlots = Math.min(
        slots,
        Math.max(0, runCapacity - currentlyActive),
      );
      const selected = run.agents
        .filter((agent) => RUNNABLE_AGENT_STATUSES.has(agent.status))
        .filter((agent) => !agent.availableAt || Date.parse(agent.availableAt) <= this.clock())
        .filter((agent) => batchIsReady(run, agent.batchId))
        .slice(0, availableSlots);
      if (selected.length === 0) return run;
      for (const agent of selected) {
        agent.status = "dispatching";
        agent.dispatchToken = randomUUID();
        agent.dispatchStartedAt = nowIso(this.clock);
        agent.attemptCount += 1;
        reservations.push({ agentId: agent.id, dispatchToken: agent.dispatchToken });
      }
      recomputeCounters(run);
      run.updatedAt = nowIso(this.clock);
      run.phase = "dispatching_switch_readonly";
      reservationEvents.push(nextEvent(run, this.clock, "switch_dispatch_reserved", {
        count: selected.length,
      }));
      return run;
    }, reservationEvents);
    if (reservations.length === 0) {
      if (reserved.agents.every((agent) => TERMINAL_AGENT_STATUSES.has(agent.status))) {
        await this.finishRun(runId);
      }
      return { runId, processed: observed.length, dispatched: 0 };
    }

    const launches = await Promise.all(reservations.map(async (reservation) => {
      const agent = reserved.agents.find(({ id }) => id === reservation.agentId);
      return {
        ...reservation,
        switchTurnId: null,
        outcome: await this.readonlyExecutor.dispatch(reserved, agent),
      };
    }));
    await this.commitSwitchOutcomes(runId, launches, "switch_dispatch_committed");
    return {
      runId,
      processed: observed.length + launches.length,
      dispatched: launches.length,
    };
  }

  async finishRun(runId) {
    const events = [];
    const run = await this.store.update(runId, (draft) => {
      recomputeCounters(draft);
      if (!["active", "paused"].includes(draft.status)) return draft;
      if (!draft.agents.every((agent) => TERMINAL_AGENT_STATUSES.has(agent.status))) return draft;
      const requiresAttention = draft.counters.failed > 0
        || draft.counters.needsAttention > 0
        || draft.counters.submitted > 0;
      draft.status = requiresAttention ? "needs_attention" : "completed";
      if (draft.executor?.kind === "switch_readonly" && draft.counters.submitted > 0) {
        draft.phase = "switch_readonly_outputs_submitted";
      } else {
        draft.phase = requiresAttention ? "phase_a_failed" : "phase_a_verified";
      }
      draft.updatedAt = nowIso(this.clock);
      draft.replaySha256 = logicalReplayHash(draft);
      const event = nextEvent(draft, this.clock, "run_finished", {
        status: draft.status,
        replaySha256: draft.replaySha256,
      });
      events.push(event);
      return draft;
    }, events);
    return run;
  }

  async cancelSwitchRun(runId) {
    if (!this.readonlyExecutor) throw new Error("Switch read-only executor is not configured");
    const requestedEvents = [];
    const paused = await this.store.update(runId, (draft) => {
      if (TERMINAL_RUN_STATUSES.has(draft.status)) throw new Error("run is already terminal");
      draft.status = "paused";
      draft.phase = "switch_cancel_requested";
      draft.updatedAt = nowIso(this.clock);
      requestedEvents.push(nextEvent(draft, this.clock, "run_cancel_requested"));
      return draft;
    }, requestedEvents);

    const remote = await Promise.all(paused.agents
      .filter((agent) => agent.status === "running" || agent.status === "dispatching")
      .map(async (agent) => {
        try {
          let turnId = agent.switchTurnId;
          if (agent.status === "dispatching") {
            const reconciled = await this.readonlyExecutor.reconcile(paused, agent);
            if (reconciled.kind !== "running") {
              return { agentId: agent.id, ok: false, reason: "dispatch_outcome_unknown" };
            }
            turnId = reconciled.snapshot.id;
          }
          const snapshot = await this.readonlyExecutor.switchClient.cancelChatTurn(turnId);
          if (String(snapshot.id) !== String(turnId)
            || snapshot.sourceChatKey !== agent.sourceChatKey
            || snapshot.status !== "cancelled") {
            return {
              agentId: agent.id,
              ok: false,
              reason: `cancel_returned_${snapshot.status ?? "invalid"}`,
            };
          }
          return { agentId: agent.id, ok: true };
        } catch (error) {
          return {
            agentId: agent.id,
            ok: false,
            reason: typeof error?.code === "string" ? error.code : "cancel_failed",
          };
        }
      }));
    const failures = remote.filter(({ ok }) => !ok);
    if (failures.length > 0) {
      const failedEvents = [];
      await this.store.update(runId, (draft) => {
        draft.phase = "switch_cancel_incomplete";
        draft.updatedAt = nowIso(this.clock);
        failedEvents.push(nextEvent(draft, this.clock, "run_cancel_incomplete", {
          failedAgentIds: failures.map(({ agentId }) => agentId).sort(),
        }));
        return draft;
      }, failedEvents);
      const error = new Error("one or more Switch turns could not be cancelled safely");
      error.code = "SWITCH_CANCEL_INCOMPLETE";
      throw error;
    }

    const completedEvents = [];
    return this.store.update(runId, (draft) => {
      draft.status = "cancelled";
      draft.phase = "cancelled";
      for (const agent of draft.agents) {
        if (!TERMINAL_AGENT_STATUSES.has(agent.status)) agent.status = "cancelled";
      }
      recomputeCounters(draft);
      draft.updatedAt = nowIso(this.clock);
      draft.replaySha256 = logicalReplayHash(draft);
      completedEvents.push(nextEvent(draft, this.clock, "run_cancel", {
        replaySha256: draft.replaySha256,
      }));
      return draft;
    }, completedEvents);
  }

  async control(runId, action) {
    const allowed = new Set(["pause", "resume", "cancel"]);
    if (!allowed.has(action)) throw new TypeError("unsupported control action");
    if (action === "cancel") {
      const current = await this.store.get(runId);
      if (current.executor?.kind === "switch_readonly") return this.cancelSwitchRun(runId);
    }
    const events = [];
    const run = await this.store.update(runId, (draft) => {
      if (TERMINAL_RUN_STATUSES.has(draft.status)) {
        throw new Error("run is already terminal");
      }
      if (action === "pause") draft.status = "paused";
      if (action === "resume") draft.status = "active";
      if (action === "cancel") {
        draft.status = "cancelled";
        draft.phase = "cancelled";
        for (const agent of draft.agents) {
          if (!TERMINAL_AGENT_STATUSES.has(agent.status)) agent.status = "cancelled";
        }
        recomputeCounters(draft);
        draft.replaySha256 = logicalReplayHash(draft);
      }
      draft.updatedAt = nowIso(this.clock);
      events.push(nextEvent(draft, this.clock, `run_${action}`, action === "cancel"
        ? { replaySha256: draft.replaySha256 }
        : {}));
      return draft;
    }, events);
    return run;
  }
}

export const schedulerInternals = Object.freeze({
  batchIsReady,
  logicalReplayHash,
  propagateDependencyFailures,
  recomputeCounters,
  TERMINAL_RUN_STATUSES,
});
