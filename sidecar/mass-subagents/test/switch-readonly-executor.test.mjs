import assert from "node:assert/strict";
import test from "node:test";

import { sha256 } from "../src/canonical.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";
import {
  SwitchReadonlyExecutor,
  buildReadonlyChatRequest,
  extractFinalText,
} from "../src/switch-readonly-executor.mjs";
import { SwitchClientError } from "../src/switch-client.mjs";

const registry = await loadRoleRegistry();
const role = registry.roles.scout;
const context = {
  run: {
    objective: "Inspect a bounded scope",
    acceptanceCriteria: ["Return a scope map"],
    constraints: ["Read only"],
    references: [],
  },
  task: {
    task: "Inspect sidecar/src",
    acceptanceCriteria: [],
    constraints: [],
    references: [],
  },
};
const contextSha256 = sha256(context);
const agent = {
  id: "agent-one",
  logicalKey: "logical-one",
  roleId: "scout",
  roleVersion: "1.0",
  roleIdentitySha256: role.identity_sha256,
  capsuleSha256: role.capsule_sha256,
  outputSchemaSha256: role.output_schema_sha256,
  contextSha256,
  executionKind: "chat",
  writeAccess: "read",
  baseCommit: "a".repeat(40),
  scope: ["sidecar/src/**"],
  sourceChatKey: "mass-subagents:test-one",
};
const run = {
  id: "run-one",
  projectDir: "E:/switch-first-commit",
  executor: {
    kind: "switch_readonly",
    mode: "ask",
    realChatDispatchEnabled: true,
  },
  definition: {
    accountId: "account-one",
    contextCatalog: { [contextSha256]: context },
    roleMaterials: {
      scout: {
        roleVersion: role.role_version,
        identitySha256: role.identity_sha256,
        capsuleSha256: role.capsule_sha256,
        capsuleSource: role.capsule_source,
        outputSchemaSha256: role.output_schema_sha256,
        outputSchema: role.output_schema,
        model: role.model,
        reasoning: role.reasoning,
      },
    },
  },
};
const validOutput = {
  schema_version: "scope-map/v1",
  base_commit: "a".repeat(40),
  cells: [{
    cell_id: "cell-one",
    scope: ["sidecar/src/**"],
    depends_on: [],
    acceptance_criteria: ["Source is inspected"],
    expected_tests: [],
    priority: 10,
    risk_class: "low",
  }],
};

const snapshot = (overrides = {}) => ({
  id: 7,
  sourceChatKey: agent.sourceChatKey,
  status: "completed",
  error: null,
  parts: [{ id: "part-1", kind: "text", status: "completed", text: JSON.stringify(validOutput) }],
  ...overrides,
});

const artifactStore = () => {
  const payloads = [];
  return {
    payloads,
    putArtifact: async (payload) => {
      payloads.push(payload);
      return { sha256: sha256(payload) };
    },
  };
};

test("read-only requests bind immutable role material and disable every write surface", () => {
  const request = buildReadonlyChatRequest(run, agent);
  assert.equal(request.mode, "ask");
  assert.equal(request.toolScope, "none");
  assert.equal(request.appWriteApproved, false);
  assert.deepEqual(request.appConnectors, []);
  assert.deepEqual(request.agentTools, []);
  assert.deepEqual(request.agentSkills, []);
  assert.equal(request.sourceChatKey, agent.sourceChatKey);
  assert.match(request.prompt, /Return exactly one JSON value/u);
  assert.match(request.prompt, new RegExp(agent.logicalKey, "u"));
  assert.throws(
    () => buildReadonlyChatRequest(run, { ...agent, writeAccess: "write-scope" }),
    /not eligible/u,
  );
});

test("completed Switch output is fully schema-validated and content addressed", async () => {
  const store = artifactStore();
  const executor = new SwitchReadonlyExecutor({
    store,
    reconciliationDelaysMs: [0],
    switchClient: {
      chatDispatchEnabled: true,
      startChatTurn: async () => snapshot(),
    },
  });
  const outcome = await executor.dispatch(run, agent);
  assert.equal(outcome.kind, "submitted");
  assert.equal(outcome.outputSha256, sha256(validOutput));
  assert.equal(store.payloads.length, 1);
  assert.deepEqual(store.payloads[0].output, validOutput);
});

test("all text parts are combined and an invalid role result needs attention", async () => {
  assert.equal(extractFinalText({ parts: [
    { kind: "text", text: " one " },
    { kind: "tool", text: "ignored" },
    { kind: "text", text: " two " },
  ] }), "one\n\ntwo");
  const store = artifactStore();
  const executor = new SwitchReadonlyExecutor({
    store,
    switchClient: {
      chatDispatchEnabled: true,
      startChatTurn: async () => snapshot({
        parts: [{ kind: "text", text: JSON.stringify({ schema_version: "scope-map/v1" }) }],
      }),
    },
  });
  const outcome = await executor.dispatch(run, agent);
  assert.equal(outcome.kind, "attention");
  assert.equal(outcome.reason, "role_output_schema_invalid");
  assert.equal(store.payloads[0].schema, "switch-mass-subagents/invalid-switch-output/v1");
});

test("an ambiguous POST is reconciled once and is never blindly replayed", async () => {
  const store = artifactStore();
  let starts = 0;
  let reconciliations = 0;
  const executor = new SwitchReadonlyExecutor({
    store,
    reconciliationDelaysMs: [0],
    switchClient: {
      chatDispatchEnabled: true,
      startChatTurn: async () => {
        starts += 1;
        throw new SwitchClientError("timeout", { code: "SWITCH_TIMEOUT" });
      },
      findActiveChatTurnBySourceChatKey: async () => {
        reconciliations += 1;
        return null;
      },
    },
  });
  const outcome = await executor.dispatch(run, agent);
  assert.equal(outcome.kind, "attention");
  assert.equal(outcome.reason, "start_outcome_unknown");
  assert.equal(starts, 1);
  assert.equal(reconciliations, 1);
});

test("Switch capacity rejection is retryable and does not need operator attention", async () => {
  const executor = new SwitchReadonlyExecutor({
    store: artifactStore(),
    reconciliationDelaysMs: [0],
    switchClient: {
      chatDispatchEnabled: true,
      startChatTurn: async () => {
        throw new SwitchClientError("capacity", {
          code: "SWITCH_HTTP_ERROR",
          status: 429,
        });
      },
    },
  });
  assert.deepEqual(await executor.dispatch(run, agent), {
    kind: "retry",
    reason: "switch_capacity",
  });
});
