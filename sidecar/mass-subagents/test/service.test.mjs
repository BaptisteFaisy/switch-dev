import assert from "node:assert/strict";
import test from "node:test";

import {
  MassSubagentService,
  SWITCH_READONLY_APPROVAL,
} from "../src/service.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";

const registry = await loadRoleRegistry();

const request = (overrides = {}) => ({
  name: "Phase A service test",
  projectDir: "E:/switch-first-commit",
  specification: {
    seed: 42,
    baseCommit: "a".repeat(40),
    context: {
      objective: "Validate the service boundary",
      acceptanceCriteria: ["Invalid scenarios are rejected before persistence"],
    },
    batches: [{
      id: "scout",
      dependsOn: [],
      assignments: [{
        id: "scout-one",
        roleId: "scout",
        roleVersion: "1.0",
        count: 1,
        scope: [],
        context: { task: "Inspect one bounded target" },
      }],
    }],
  },
  ...overrides,
});

const harness = (options = {}) => {
  const records = [];
  const store = {
    create: async (record) => { records.push(record); },
    list: async () => ({ items: records, nextCursor: null, total: records.length }),
    get: async (id) => structuredClone(records.find((record) => record.id === id)),
  };
  const scheduler = {
    capacity: 2,
    microtasksPerSession: 32,
  };
  return {
    records,
    service: new MassSubagentService({ store, scheduler, registry, ...options }),
  };
};

test("invalid fake scenarios are rejected before a run can be persisted", async () => {
  const { records, service } = harness();
  await assert.rejects(
    () => service.create(request({ fakeScenario: { throttleEvery: -1 } })),
    /throttleEvery/u,
  );
  await assert.rejects(
    () => service.create(request({ fakeScenario: { unsupportedFault: 2 } })),
    /unknown fields/u,
  );
  assert.equal(records.length, 0);
});

test("real dispatch requires deployment enablement, per-run approval and an exact budget", async () => {
  const disabled = harness();
  await assert.rejects(
    () => disabled.service.create(request({
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "disabled-one",
      maxChatTurns: 1,
      realDispatchApproval: SWITCH_READONLY_APPROVAL,
    })),
    (error) => error.code === "REAL_DISPATCH_DISABLED",
  );
  const enabled = harness({
    realDispatchEnabled: true,
    maxRealAgents: 4,
    switchClient: { authenticated: true, chatDispatchEnabled: true },
  });
  await assert.rejects(
    () => enabled.service.create(request({
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "approval-one",
      maxChatTurns: 1,
    })),
    (error) => error.code === "REAL_DISPATCH_APPROVAL_REQUIRED",
  );
  await assert.rejects(
    () => enabled.service.create(request({
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "budget-one",
      maxChatTurns: 2,
      realDispatchApproval: SWITCH_READONLY_APPROVAL,
    })),
    (error) => error.code === "REAL_DISPATCH_BUDGET_MISMATCH",
  );
  assert.equal(enabled.records.length, 0);
});

test("switch_readonly persists bounded role material but refuses every writer", async () => {
  const enabled = harness({
    realDispatchEnabled: true,
    maxRealAgents: 4,
    switchClient: { authenticated: true, chatDispatchEnabled: true },
  });
  await enabled.service.create(request({
    executor: "switch_readonly",
    accountId: "account-one",
    idempotencyKey: "request-one",
    maxChatTurns: 1,
    realDispatchApproval: SWITCH_READONLY_APPROVAL,
  }));
  assert.equal(enabled.records[0].executor.kind, "switch_readonly");
  assert.equal(enabled.records[0].executor.mode, "ask");
  assert.equal(enabled.records[0].definition.accountId, "account-one");
  assert.equal(enabled.records[0].definition.roleMaterials.scout.capsuleSource.length > 0, true);
  assert.equal(Object.keys(enabled.records[0].definition.contextCatalog).length, 1);
  const replay = await enabled.service.create(request({
    executor: "switch_readonly",
    accountId: "account-one",
    idempotencyKey: "request-one",
    maxChatTurns: 1,
    realDispatchApproval: SWITCH_READONLY_APPROVAL,
  }));
  assert.equal(replay.idempotentReplay, true);
  assert.equal(enabled.records.length, 1);
  await assert.rejects(
    () => enabled.service.create(request({
      name: "Changed request",
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "request-one",
      maxChatTurns: 1,
      realDispatchApproval: SWITCH_READONLY_APPROVAL,
    })),
    (error) => error.code === "REAL_DISPATCH_IDEMPOTENCY_CONFLICT",
  );
  await assert.rejects(
    () => enabled.service.create(request({
      executor: "switch_readonly",
      accountId: "account-one",
      idempotencyKey: "request-two",
      maxChatTurns: 1,
      realDispatchApproval: SWITCH_READONLY_APPROVAL,
    })),
    (error) => error.code === "REAL_DISPATCH_RUN_CONFLICT",
  );

  const writer = request({
    executor: "switch_readonly",
    accountId: "account-one",
    idempotencyKey: "writer-one",
    maxChatTurns: 1,
    realDispatchApproval: SWITCH_READONLY_APPROVAL,
  });
  writer.specification.batches[0].assignments[0].roleId = "implementer";
  writer.specification.batches[0].assignments[0].scope = ["sidecar/src/service.mjs"];
  await assert.rejects(
    () => enabled.service.create(writer),
    (error) => error.code === "REAL_DISPATCH_ROLE_REFUSED",
  );
});

test("the service persists a normalized replay-stable fake scenario", async () => {
  const { records, service } = harness();
  await service.create(request({ fakeScenario: { duplicateEvery: 3 } }));
  assert.equal(records.length, 1);
  assert.deepEqual(records[0].definition.fakeScenario, {
    throttleEvery: 0,
    timeoutEvery: 0,
    duplicateEvery: 3,
    outOfOrderEvery: 0,
    latencyEvery: 0,
    latencyMs: 0,
  });
  assert.equal(records[0].executor.realChatDispatchEnabled, false);
});
