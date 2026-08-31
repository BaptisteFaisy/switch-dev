import assert from "node:assert/strict";
import { mkdtemp, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { cliInternals } from "../src/cli.mjs";
import { MassSubagentScheduler } from "../src/scheduler.mjs";
import { MassSubagentService, SWITCH_READONLY_APPROVAL } from "../src/service.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";
import { JsonRunStore } from "../src/store.mjs";
import { SwitchClientError } from "../src/switch-client.mjs";

const registry = await loadRoleRegistry();
const validOutput = {
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

const request = (count = 1) => ({
  name: "Switch no-tool safety",
  projectDir: "E:/switch-first-commit",
  executor: "switch_readonly",
  accountId: "account-one",
  idempotencyKey: "switch-safety-" + count,
  maxChatTurns: count,
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
        count,
        scope: ["sidecar/src/**"],
        context: { task: "Inspect the bounded source tree" },
      }],
    }],
  },
});

const withRealHarness = async (switchClient, callback, {
  count = 1,
  clock = Date.now,
  maxAttempts = 3,
  maxPollErrors = 2,
  maxStoredRuns = 100,
} = {}) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-safety-"));
  try {
    const store = await new JsonRunStore(root).init();
    const scheduler = new MassSubagentScheduler({
      store,
      capacity: 2,
      intervalMs: 1_000,
      maxAttempts,
      maxPollErrors,
      switchClient,
      clock,
    });
    const service = new MassSubagentService({
      store,
      scheduler,
      registry,
      switchClient,
      realDispatchEnabled: true,
      maxRealAgents: 4,
      maxStoredRuns,
    });
    const candidate = request(count);
    const created = await service.create(candidate);
    await callback({ candidate, created, scheduler, service, store });
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

test("real mode refuses an older Switch runtime that cannot prove no-tool support", () => {
  assert.doesNotThrow(() => cliInternals.assertNoToolScopeCapability({
    chatTurnToolScopes: ["full", "none"],
  }));
  assert.throws(
    () => cliInternals.assertNoToolScopeCapability({ ready: true }),
    /does not advertise chat toolScope=none/u,
  );
  assert.throws(
    () => cliInternals.assertNoToolScopeCapability({ chatTurnToolScopes: ["full"] }),
    /does not advertise chat toolScope=none/u,
  );
});

test("ordinary Switch traffic is subtracted from the real dispatch capacity", async () => {
  let nextId = 10;
  let concurrentStarts = 0;
  let peakStarts = 0;
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
    listActiveChatTurns: async () => [{
      id: 999,
      status: "running",
      sourceChatKey: "ordinary-switch-chat",
    }],
    findActiveChatTurnBySourceChatKey: async () => null,
    getChatTurn: async () => { throw new Error("completed turns are not polled"); },
    cancelChatTurn: async () => { throw new Error("nothing to cancel"); },
    startChatTurn: async (chatRequest) => {
      concurrentStarts += 1;
      peakStarts = Math.max(peakStarts, concurrentStarts);
      await new Promise((resolve) => setTimeout(resolve, 5));
      concurrentStarts -= 1;
      nextId += 1;
      return {
        id: nextId,
        sourceChatKey: chatRequest.sourceChatKey,
        status: "completed",
        parts: [{ kind: "text", text: JSON.stringify(validOutput) }],
      };
    },
  };
  await withRealHarness(switchClient, async ({ created, scheduler, store }) => {
    await scheduler.tickSwitchRun(created.id);
    assert.equal((await store.get(created.id)).counters.submitted, 1);
    await scheduler.tickSwitchRun(created.id);
    const run = await store.get(created.id);
    assert.equal(run.counters.submitted, 2);
    assert.equal(peakStarts, 1);
  }, { count: 2 });
});

test("persistent Switch capacity races are bounded instead of retrying forever", async () => {
  let now = Date.parse("2026-08-31T00:00:00.000Z");
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
    listActiveChatTurns: async () => [],
    findActiveChatTurnBySourceChatKey: async () => null,
    startChatTurn: async () => {
      throw new SwitchClientError("capacity", {
        code: "SWITCH_HTTP_ERROR",
        status: 429,
      });
    },
    getChatTurn: async () => { throw new Error("unused"); },
    cancelChatTurn: async () => { throw new Error("unused"); },
  };
  await withRealHarness(switchClient, async ({ created, scheduler, store }) => {
    for (let attempt = 0; attempt < 3; attempt += 1) {
      await scheduler.tickSwitchRun(created.id);
      now += 1_000;
    }
    const run = await store.get(created.id);
    assert.equal(run.status, "needs_attention");
    assert.equal(run.agents[0].attemptCount, 3);
    assert.equal(run.agents[0].attentionReason, "switch_capacity_retries_exhausted");
    assert.equal(run.counters.retries, 3);
  }, { clock: () => now, maxAttempts: 3 });
});

test("persistent Switch poll failures become terminal attention instead of wedging forever", async () => {
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
    listActiveChatTurns: async () => [{
      id: 41,
      status: "running",
      sourceChatKey: "active",
    }],
    findActiveChatTurnBySourceChatKey: async () => null,
    startChatTurn: async (chatRequest) => ({
      id: 41,
      sourceChatKey: chatRequest.sourceChatKey,
      status: "running",
    }),
    getChatTurn: async () => {
      throw new SwitchClientError("revoked", { code: "SWITCH_HTTP_ERROR", status: 401 });
    },
    cancelChatTurn: async () => { throw new Error("unused"); },
  };
  await withRealHarness(switchClient, async ({ created, scheduler, store }) => {
    await scheduler.tickSwitchRun(created.id);
    await scheduler.tickSwitchRun(created.id);
    assert.equal((await store.get(created.id)).status, "active");
    await scheduler.tickSwitchRun(created.id);
    const run = await store.get(created.id);
    assert.equal(run.status, "needs_attention");
    assert.equal(run.agents[0].pollErrorCount, 2);
    assert.equal(run.agents[0].attentionReason, "persistent_SWITCH_HTTP_ERROR");
  });
});

test("cancellation refuses a finalizing response instead of claiming remote cancellation", async () => {
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
    listActiveChatTurns: async () => [],
    findActiveChatTurnBySourceChatKey: async () => null,
    startChatTurn: async (chatRequest) => ({
      id: 52,
      sourceChatKey: chatRequest.sourceChatKey,
      status: "running",
    }),
    getChatTurn: async () => { throw new Error("unused"); },
    cancelChatTurn: async (_turnId) => ({
      id: 52,
      sourceChatKey: "wrong-key-is-rejected",
      status: "finalizing",
    }),
  };
  await withRealHarness(switchClient, async ({ created, scheduler, store }) => {
    await scheduler.tickSwitchRun(created.id);
    await assert.rejects(
      () => scheduler.control(created.id, "cancel"),
      { code: "SWITCH_CANCEL_INCOMPLETE" },
    );
    const run = await store.get(created.id);
    assert.equal(run.status, "paused");
    assert.equal(run.phase, "switch_cancel_incomplete");
    assert.notEqual(run.agents[0].status, "cancelled");
  });
});

test("an exact idempotent retry wins over the retention limit", async () => {
  const switchClient = {
    authenticated: true,
    chatDispatchEnabled: true,
  };
  await withRealHarness(switchClient, async ({ candidate, service }) => {
    const replay = await service.create(candidate);
    assert.equal(replay.idempotentReplay, true);
  }, { maxStoredRuns: 1 });
});

test("scheduler stop waits for the active tick to leave its critical work", async () => {
  let enter;
  let release;
  const entered = new Promise((resolve) => { enter = resolve; });
  const blocked = new Promise((resolve) => { release = resolve; });
  const scheduler = new MassSubagentScheduler({
    capacity: 1,
    store: {
      list: async () => {
        enter();
        await blocked;
        return { items: [], nextCursor: null };
      },
    },
  });
  const tick = scheduler.tick();
  await entered;
  let stopped = false;
  const stopping = scheduler.stop().then(() => { stopped = true; });
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(stopped, false);
  release();
  await Promise.all([tick, stopping]);
  assert.equal(stopped, true);
});
