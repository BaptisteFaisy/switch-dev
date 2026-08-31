import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { MassSubagentScheduler } from "../src/scheduler.mjs";
import { MassSubagentService, SWITCH_READONLY_APPROVAL } from "../src/service.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";
import { JsonRunStore } from "../src/store.mjs";

const registry = await loadRoleRegistry();

const record = (count = 10) => ({
  id: "run-scheduler",
  createdAt: "2026-08-31T00:00:00.000Z",
  updatedAt: "2026-08-31T00:00:00.000Z",
  status: "active",
  phase: "queued",
  configSha256: "a".repeat(64),
  definition: {
    seed: 42,
    fakeScenario: { throttleEvery: 3, duplicateEvery: 4 },
  },
  executor: { kind: "fake", capacity: 2 },
  batches: [{ id: "batch", dependsOn: [] }],
  agents: Array.from({ length: count }, (_, index) => ({
    id: `agent-${index + 1}`,
    logicalKey: `logical-${index + 1}`,
    ordinal: index + 1,
    batchId: "batch",
    status: "queued",
    attemptCount: 0,
    idempotencyKey: `run:agent-${index + 1}`,
  })),
  counters: {
    queued: count,
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

const withScheduler = async (callback, { maxAttempts = 3 } = {}) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-scheduler-"));
  try {
    const store = await new JsonRunStore(root).init();
    let now = Date.parse("2026-08-31T00:00:00.000Z");
    const scheduler = new MassSubagentScheduler({
      store,
      capacity: 2,
      microtasksPerSession: 2,
      maxAttempts,
      clock: () => now,
    });
    await callback({ store, scheduler, advance: (milliseconds = 100) => { now += milliseconds; } });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

test("phase A scheduler batches logical agents under the Switch capacity", async () => {
  await withScheduler(async ({ store, scheduler, advance }) => {
    await store.create(record(), [{ sequence: 1, kind: "created" }]);
    for (let iteration = 0; iteration < 10; iteration += 1) {
      await scheduler.tickRun("run-scheduler");
      advance();
      if ((await store.get("run-scheduler")).status === "completed") break;
    }
    const run = await store.get("run-scheduler");
    assert.equal(run.status, "completed");
    assert.equal(run.counters.verified, 10);
    assert.ok(run.counters.retries > 0);
    assert.ok(run.counters.duplicatesIgnored > 0);
    assert.ok(run.counters.peakSessions <= 2);
    assert.match(run.replaySha256, /^[a-f0-9]{64}$/u);
    const events = await store.readEvents(run.id);
    assert.equal(events[0].kind, "created");
    assert.ok(events.some(({ kind }) => kind === "fake_wave_committed"));
    assert.equal(events.at(-1).kind, "run_finished");
    assert.deepEqual(
      events.map(({ sequence }) => sequence),
      Array.from({ length: events.length }, (_, index) => index + 1),
    );
  });
});

test("a failed dependency terminates its downstream DAG instead of starving the run", async () => {
  await withScheduler(async ({ store, scheduler }) => {
    const run = record(2);
    run.definition.fakeScenario = { throttleEvery: 1 };
    run.batches = [
      { id: "first", dependsOn: [] },
      { id: "second", dependsOn: ["first"] },
    ];
    run.agents[0].batchId = "first";
    run.agents[1].batchId = "second";
    await store.create(run, [{ sequence: 1, kind: "created" }]);
    await scheduler.tickRun(run.id);

    const failed = await store.get(run.id);
    assert.equal(failed.status, "needs_attention");
    assert.equal(failed.agents[0].status, "failed");
    assert.equal(failed.agents[1].status, "failed");
    assert.equal(failed.agents[1].failureReason, "dependency_failed");
    assert.equal(failed.counters.queued, 0);
    assert.equal(failed.counters.failed, 2);
    assert.deepEqual(
      (await store.readEvents(run.id)).map(({ kind }) => kind),
      ["created", "fake_wave_committed", "dependent_batches_blocked", "run_finished"],
    );
    for (const action of ["pause", "resume", "cancel"]) {
      await assert.rejects(() => scheduler.control(run.id, action), /already terminal/u);
    }
  }, { maxAttempts: 1 });
});

test("DAG dependencies prevent a downstream batch from running early", async () => {
  await withScheduler(async ({ store, scheduler, advance }) => {
    const run = record(2);
    run.batches = [
      { id: "first", dependsOn: [] },
      { id: "second", dependsOn: ["first"] },
    ];
    run.agents[0].batchId = "first";
    run.agents[1].batchId = "second";
    await store.create(run);
    await scheduler.tickRun(run.id);
    advance();
    const afterFirst = await store.get(run.id);
    assert.equal(afterFirst.agents[0].status, "verified");
    assert.equal(afterFirst.agents[1].status, "queued");
    await scheduler.tickRun(run.id);
    const afterSecond = await store.get(run.id);
    assert.equal(afterSecond.agents[1].status, "verified");
  });
});

test("cancel is terminal and releases every queued logical agent", async () => {
  await withScheduler(async ({ store, scheduler }) => {
    await store.create(record());
    await scheduler.control("run-scheduler", "cancel");
    const run = await store.get("run-scheduler");
    assert.equal(run.status, "cancelled");
    assert.equal(run.counters.cancelled, 10);
    assert.match(run.replaySha256, /^[a-f0-9]{64}$/u);
    assert.equal((await scheduler.tickRun(run.id)).processed, 0);
    assert.equal((await store.readEvents(run.id)).at(-1).kind, "run_cancel");
  });
});

test("concurrent scheduler instances commit each fake wave only once", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-scheduler-concurrent-"));
  try {
    const firstStore = await new JsonRunStore(root).init();
    const secondStore = await new JsonRunStore(root).init();
    const run = record(4);
    run.definition.fakeScenario = {};
    await firstStore.create(run, [{ sequence: 1, kind: "created" }]);
    const schedulers = [firstStore, secondStore].map((store) => new MassSubagentScheduler({
      store,
      capacity: 2,
      microtasksPerSession: 2,
      clock: () => Date.parse("2026-08-31T00:00:00.000Z"),
    }));
    await Promise.all(schedulers.map((scheduler) => scheduler.tickRun(run.id)));
    const completed = await firstStore.get(run.id);
    assert.equal(completed.status, "completed");
    assert.equal(completed.counters.verified, 4);
    assert.equal(completed.counters.duplicatesIgnored, 0);
    assert.equal(completed.counters.sessionWaves, 1);
    assert.equal(completed.counters.sessionsUsed, 2);
    assert.equal(
      (await firstStore.readEvents(run.id)).filter(({ kind }) => kind === "fake_wave_committed").length,
      1,
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("switch_readonly launches real chat DTOs outside the store lock under global capacity", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-readonly-"));
  let inFlight = 0;
  let peak = 0;
  const requests = [];
  const output = {
    schema_version: "scope-map/v1",
    base_commit: "b".repeat(40),
    cells: [{
      cell_id: "bounded-cell",
      scope: ["sidecar/src/**"],
      depends_on: [],
      acceptance_criteria: ["The scope was inspected"],
      expected_tests: [],
      priority: 10,
      risk_class: "low",
    }],
  };
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
    listActiveChatTurns: async () => [],
    startChatTurn: async (request) => {
      requests.push(request);
      inFlight += 1;
      peak = Math.max(peak, inFlight);
      await new Promise((resolve) => setTimeout(resolve, 10));
      inFlight -= 1;
      return {
        id: requests.length,
        sourceChatKey: request.sourceChatKey,
        status: "completed",
        error: null,
        parts: [{ kind: "text", text: JSON.stringify(output) }],
      };
    },
    getChatTurn: async () => { throw new Error("completed turns are not polled"); },
    findActiveChatTurnBySourceChatKey: async () => null,
    cancelChatTurn: async () => { throw new Error("nothing to cancel"); },
  };
  try {
    const store = await new JsonRunStore(root).init();
    const scheduler = new MassSubagentScheduler({
      store,
      capacity: 2,
      intervalMs: 1_000,
      switchClient,
    });
    const service = new MassSubagentService({
      store,
      scheduler,
      registry,
      switchClient,
      realDispatchEnabled: true,
      maxRealAgents: 5,
    });
    const created = await service.create({
      name: "Read-only Switch pilot",
      projectDir: "E:/switch-first-commit",
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "scheduler-pilot-one",
      maxChatTurns: 5,
      realDispatchApproval: SWITCH_READONLY_APPROVAL,
      capacity: 2,
      specification: {
        seed: 42,
        baseCommit: "b".repeat(40),
        context: {
          objective: "Inspect a bounded project",
          acceptanceCriteria: ["Every scout returns a strict scope map"],
        },
        batches: [{
          id: "scouts",
          dependsOn: [],
          assignments: [{
            id: "bounded-readers",
            roleId: "scout",
            roleVersion: "1.0",
            count: 5,
            scope: ["sidecar/src/**"],
            context: { task: "Inspect the bounded source tree" },
          }],
        }],
      },
    });
    for (let iteration = 0; iteration < 4; iteration += 1) {
      await scheduler.tickSwitchRun(created.id);
      if ((await store.get(created.id)).status === "needs_attention") break;
    }
    const run = await store.get(created.id);
    assert.equal(run.status, "needs_attention");
    assert.equal(run.phase, "switch_readonly_outputs_submitted");
    assert.equal(run.counters.submitted, 5);
    assert.equal(run.counters.verified, 0);
    assert.equal(requests.length, 5);
    assert.equal(peak, 2);
    assert.ok(requests.every((request) => request.mode === "ask"));
    assert.ok(requests.every((request) => request.toolScope === "none"));
    assert.ok(requests.every((request) => request.appWriteApproved === false));
    assert.ok(run.agents.every((candidate) =>
      candidate.verificationStage === "role_output_schema"));
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("cancelling a real run first cancels every known Switch turn", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-readonly-cancel-"));
  const cancelled = [];
  try {
    const store = await new JsonRunStore(root).init();
    const run = record(1);
    run.executor = {
      kind: "switch_readonly",
      capacity: 1,
      mode: "ask",
      realChatDispatchEnabled: true,
    };
    run.agents[0].status = "running";
    run.agents[0].switchTurnId = 41;
    run.agents[0].sourceChatKey = "mass-subagents:cancel-one";
    run.counters.queued = 0;
    run.counters.running = 1;
    await store.create(run, [{ sequence: 1, kind: "created" }]);
    const scheduler = new MassSubagentScheduler({
      store,
      capacity: 1,
      switchClient: {
        chatDispatchEnabled: true,
        cancelChatTurn: async (turnId) => {
          cancelled.push(turnId);
          return { id: turnId, status: "cancelled", sourceChatKey: "mass-subagents:cancel-one" };
        },
      },
    });
    await scheduler.control(run.id, "cancel");
    const result = await store.get(run.id);
    assert.deepEqual(cancelled, [41]);
    assert.equal(result.status, "cancelled");
    assert.equal(result.counters.cancelled, 1);
    assert.deepEqual(
      (await store.readEvents(run.id)).map(({ kind }) => kind),
      ["created", "run_cancel_requested", "run_cancel"],
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});
