import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import { sha256 } from "../src/canonical.mjs";
import { AgentFactory, RUN_SPEC_SCHEMA_VERSION } from "../src/factory.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";
import { MassSubagentScheduler } from "../src/scheduler.mjs";
import { JsonRunStore } from "../src/store.mjs";

const parseArguments = (values) => {
  const options = { count: 10_000, repeats: 2, capacity: 2, microtasksPerSession: 256 };
  for (let index = 0; index < values.length; index += 2) {
    const name = values[index];
    const value = Number(values[index + 1]);
    if (values[index + 1] === undefined || !Number.isInteger(value)) {
      throw new TypeError("every option requires an integer value");
    }
    if (name === "--count") options.count = value;
    else if (name === "--repeats") options.repeats = value;
    else if (name === "--capacity") options.capacity = value;
    else if (name === "--microtasks-per-session") options.microtasksPerSession = value;
    else throw new TypeError("unknown option " + name);
  }
  if (options.count < 1 || options.count > 10_000) {
    throw new TypeError("count must be between 1 and 10000");
  }
  if (options.repeats < 1 || options.repeats > 2) {
    throw new TypeError("repeats must be 1 or 2");
  }
  if (options.capacity < 1 || options.capacity > 32) {
    throw new TypeError("capacity must be between 1 and 32");
  }
  if (options.microtasksPerSession < 1 || options.microtasksPerSession > 256) {
    throw new TypeError("microtasks-per-session must be between 1 and 256");
  }
  return options;
};

const specification = ({ count, runId }) => ({
  schemaVersion: RUN_SPEC_SCHEMA_VERSION,
  runId,
  seed: 42,
  baseCommit: "a".repeat(40),
  context: {
    objective: "Exercise ten thousand bounded logical agents with the fake provider",
    acceptanceCriteria: ["Every logical agent reaches a verified terminal state"],
    constraints: ["Switch development only", "No network or real provider"],
  },
  batches: [{
    id: "smoke-scouts",
    dependsOn: [],
    assignments: [{
      id: "bounded-readers",
      roleId: "scout",
      roleVersion: "1.0",
      count,
      scope: ["sidecar/src/**"],
      context: {
        task: "Return one deterministic fake-provider result",
        acceptanceCriteria: ["Remain inside the configured scheduler capacity"],
      },
    }],
  }],
});

const runtimeRecord = ({ plan, scenario, capacity, microtasksPerSession }) => ({
  id: plan.runId,
  name: "10k fake-provider smoke",
  createdAt: "2026-08-31T00:00:00.000Z",
  updatedAt: "2026-08-31T00:00:00.000Z",
  status: "active",
  phase: "queued",
  logicalAgentCount: plan.logicalAgentCount,
  configSha256: sha256({ planSha256: plan.planSha256, scenario }),
  planSha256: plan.planSha256,
  definition: { seed: plan.seed, fakeScenario: scenario },
  executor: { kind: "fake", capacity, microtasksPerSession },
  batches: plan.batches.map((batch) => ({
    id: batch.id,
    dependsOn: batch.dependsOn,
  })),
  agents: plan.agents.map((agent) => ({
    id: agent.id,
    logicalKey: agent.logicalKey,
    ordinal: agent.ordinal,
    batchId: agent.batchId,
    status: "queued",
    attemptCount: 0,
    idempotencyKey: agent.idempotencyKey,
  })),
  counters: {
    queued: plan.logicalAgentCount,
    retryWait: 0,
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
});

const execute = async ({ registry, options, repetition }) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-10000-"));
  const startedAt = performance.now();
  let peakRssBytes = process.memoryUsage().rss;
  try {
    const plan = new AgentFactory({ registry }).expand(specification({
      count: options.count,
      runId: "run-smoke-10000-" + repetition,
    }));
    const scenario = {
      throttleEvery: 37,
      timeoutEvery: 53,
      duplicateEvery: 17,
      outOfOrderEvery: 13,
      latencyEvery: 11,
      latencyMs: 25,
    };
    const store = await new JsonRunStore(root).init();
    await store.create(runtimeRecord({
      plan,
      scenario,
      capacity: options.capacity,
      microtasksPerSession: options.microtasksPerSession,
    }), [{ sequence: 1, timestamp: "2026-08-31T00:00:00.000Z", kind: "run_created" }]);
    let now = Date.parse("2026-08-31T00:00:00.000Z");
    const scheduler = new MassSubagentScheduler({
      store,
      capacity: options.capacity,
      microtasksPerSession: options.microtasksPerSession,
      clock: () => now,
    });
    let run;
    for (let iteration = 0; iteration < 1_000; iteration += 1) {
      await scheduler.tickRun(plan.runId);
      run = await store.get(plan.runId);
      peakRssBytes = Math.max(peakRssBytes, process.memoryUsage().rss);
      if (["completed", "needs_attention"].includes(run.status)) break;
      now += 1_000;
    }
    if (run?.status !== "completed") throw new Error("10k smoke did not converge");
    if (run.counters.verified !== options.count) throw new Error("verified count mismatch");
    if (run.counters.peakSessions > options.capacity) throw new Error("capacity was exceeded");
    return {
      logicalAgentCount: run.logicalAgentCount,
      verified: run.counters.verified,
      failed: run.counters.failed,
      retries: run.counters.retries,
      duplicatesIgnored: run.counters.duplicatesIgnored,
      sessionWaves: run.counters.sessionWaves,
      sessionsUsed: run.counters.sessionsUsed,
      peakSessions: run.counters.peakSessions,
      planSha256: run.planSha256,
      replaySha256: run.replaySha256,
      durationMs: Math.round(performance.now() - startedAt),
      peakRssBytes,
    };
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

const main = async () => {
  if (process.env.SWITCH_ENV !== "development") {
    throw new Error("SWITCH_ENV=development is mandatory");
  }
  if (process.env.MASS_SUBAGENTS_REAL_ENABLED === "true") {
    throw new Error("the 10k smoke refuses real dispatch");
  }
  const options = parseArguments(process.argv.slice(2));
  const registry = await loadRoleRegistry();
  const results = [];
  for (let repetition = 1; repetition <= options.repeats; repetition += 1) {
    results.push(await execute({ registry, options, repetition }));
  }
  if (new Set(results.map((result) => result.planSha256)).size !== 1) {
    throw new Error("plan hash changed across deterministic repetitions");
  }
  if (new Set(results.map((result) => result.replaySha256)).size !== 1) {
    throw new Error("replay hash changed across deterministic repetitions");
  }
  process.stdout.write(JSON.stringify({
    status: "passed",
    provider: "fake",
    networkUsed: false,
    ...options,
    results,
  }) + "\n");
};

main().catch((error) => {
  process.stderr.write(error.message + "\n");
  process.exitCode = 1;
});
