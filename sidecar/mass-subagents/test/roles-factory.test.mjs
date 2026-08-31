import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  AgentFactory,
  MAX_COORDINATORS,
  MAX_LOGICAL_AGENTS,
  RUN_SPEC_SCHEMA_VERSION,
  validateBatchGraph,
} from "../src/factory.mjs";
import {
  ROLE_ARCHETYPES,
  ROLE_IDS,
  loadRoleRegistry,
  validateRoleManifest,
} from "../src/roles.mjs";

const registry = await loadRoleRegistry();

const assignment = ({
  id,
  roleId,
  count = 1,
  scope,
  scopeSets,
  task = id,
}) => ({
  id,
  roleId,
  roleVersion: "1.0",
  count,
  ...(scopeSets ? { scopeSets } : { scope: scope ?? [] }),
  context: {
    task,
    acceptanceCriteria: [`${task} is verified`],
  },
});

const runSpec = () => ({
  schemaVersion: RUN_SPEC_SCHEMA_VERSION,
  runId: "run-factory-001",
  seed: 42,
  baseCommit: "a".repeat(40),
  context: {
    objective: "Build a deterministic logical-agent plan",
    acceptanceCriteria: ["Every generated identity is reproducible"],
    constraints: ["Development only"],
  },
  batches: [
    {
      id: "writers",
      dependsOn: [],
      assignments: [assignment({
        id: "implementation-cells",
        roleId: "implementer",
        count: 2,
        scopeSets: [["sidecar/src/alpha.mjs"], ["sidecar/src/beta.mjs"]],
      })],
    },
    {
      id: "tests",
      dependsOn: ["writers"],
      assignments: [assignment({
        id: "alpha-tests",
        roleId: "test_writer",
        scope: ["sidecar/src/alpha.mjs"],
      })],
    },
    {
      id: "review",
      dependsOn: ["tests"],
      assignments: [
        assignment({
          id: "adversarial-check",
          roleId: "adversary",
          scope: ["sidecar/src/**"],
        }),
        assignment({
          id: "quality-gate",
          roleId: "quality_reviewer",
          scope: ["sidecar/src/**"],
        }),
      ],
    },
    {
      id: "integration",
      dependsOn: ["review"],
      assignments: [assignment({
        id: "main-commit-lane",
        roleId: "integrator",
        scope: [],
      })],
    },
  ],
});

test("the registry loads exactly the seven versioned roles and their T1-T5/CAS mapping", () => {
  assert.deepEqual(Object.keys(registry.roles), ROLE_IDS);
  assert.match(registry.registry_sha256, /^[a-f0-9]{64}$/u);
  for (const roleId of ROLE_IDS) {
    const role = registry.roles[roleId];
    assert.equal(role.archetype_id, ROLE_ARCHETYPES[roleId].archetypeId);
    assert.equal(role.archetype, ROLE_ARCHETYPES[roleId].archetype);
    assert.match(role.identity_sha256, /^[a-f0-9]{64}$/u);
  }
  assert.equal(registry.roles.integrator.execution_kind, "controlled-code");
  assert.equal(registry.roles.integrator.archetype_id, "CAS");
});

test("role manifests reject unknown fields and role/archetype mismatches", async () => {
  const manifestUrl = new URL("../agent_roles/scout/v1/manifest.json", import.meta.url);
  const manifest = JSON.parse(await readFile(manifestUrl, "utf8"));
  assert.throws(
    () => validateRoleManifest({ ...manifest, unexpected: true }),
    /extra: unexpected/u,
  );
  assert.throws(
    () => validateRoleManifest({ ...manifest, archetype_id: "T2" }),
    /archetype_id must be T1/u,
  );
});

test("factory expansion is deterministic across input batch and assignment ordering", () => {
  const factory = new AgentFactory({ registry });
  const firstSpec = runSpec();
  const secondSpec = runSpec();
  secondSpec.batches.reverse();
  secondSpec.batches.find(({ id }) => id === "review").assignments.reverse();

  const first = factory.expand(firstSpec);
  const second = factory.expand(secondSpec);
  assert.deepEqual(second, first);
  assert.equal(first.logicalAgentCount, 6);
  assert.deepEqual(first.batches.map(({ id }) => id), [
    "writers",
    "tests",
    "review",
    "integration",
  ]);
  assert.equal(new Set(first.agents.map(({ id }) => id)).size, first.agents.length);
  assert.equal(
    new Set(first.agents.map(({ idempotencyKey }) => idempotencyKey)).size,
    first.agents.length,
  );
  assert.match(first.planSha256, /^[a-f0-9]{64}$/u);
  assert.ok(Object.isFrozen(first));
  assert.ok(Object.isFrozen(first.agents[0].context));
});

test("replay identities and plan hash are stable across distinct run ids", () => {
  const factory = new AgentFactory({ registry });
  const firstSpec = runSpec();
  const secondSpec = runSpec();
  secondSpec.runId = "run-factory-002";

  const first = factory.expand(firstSpec);
  const second = factory.expand(secondSpec);
  assert.equal(second.planSha256, first.planSha256);
  assert.notEqual(second.instancePlanSha256, first.instancePlanSha256);
  assert.deepEqual(
    second.agents.map(({ logicalKey }) => logicalKey),
    first.agents.map(({ logicalKey }) => logicalKey),
  );
  assert.notDeepEqual(
    second.agents.map(({ id }) => id),
    first.agents.map(({ id }) => id),
  );
  assert.notDeepEqual(
    second.agents.map(({ idempotencyKey }) => idempotencyKey),
    first.agents.map(({ idempotencyKey }) => idempotencyKey),
  );
});

test("one batch expands to the hard ceiling of 10000 logical agents", () => {
  const specification = runSpec();
  specification.batches = [{
    id: "mutation-volume",
    dependsOn: [],
    assignments: [assignment({
      id: "mutants",
      roleId: "mutation_operator",
      count: MAX_LOGICAL_AGENTS,
      scope: ["sidecar/src/**"],
    })],
  }];
  const plan = new AgentFactory({ registry }).expand(specification);
  assert.equal(plan.logicalAgentCount, MAX_LOGICAL_AGENTS);
  assert.equal(plan.agents.length, MAX_LOGICAL_AGENTS);
  assert.equal(new Set(plan.agents.map(({ logicalKey }) => logicalKey)).size, MAX_LOGICAL_AGENTS);
  assert.ok(plan.agents.every(({ executionKind }) => executionKind === "chat"));
});

test("factory rejects a run above 10000 logical agents", () => {
  const specification = runSpec();
  specification.batches = [{
    id: "too-many",
    dependsOn: [],
    assignments: [
      assignment({
        id: "first-volume",
        roleId: "mutation_operator",
        count: MAX_LOGICAL_AGENTS,
      }),
      assignment({ id: "one-more", roleId: "scout" }),
    ],
  }];
  assert.throws(
    () => new AgentFactory({ registry }).expand(specification),
    /beyond 10000 logical agents/u,
  );
});

test("the coordinator fan-out is bounded to 32", () => {
  const specification = runSpec();
  specification.batches = Array.from({ length: MAX_COORDINATORS + 1 }, (_, index) => ({
    id: `cell-${index + 1}`,
    dependsOn: [],
    assignments: [assignment({
      id: "reader",
      roleId: "scout",
      task: `Inspect cell ${index + 1}`,
    })],
  }));
  assert.throws(
    () => new AgentFactory({ registry }).expand(specification),
    /between 1 and 32 coordinator cells/u,
  );
});

test("inherited context is bounded and does not include the whole run plan", () => {
  const specification = runSpec();
  assert.throws(
    () => new AgentFactory({ registry, maxContextBytes: 64 }).expand(specification),
    /bounded context/u,
  );
  const plan = new AgentFactory({ registry }).expand(specification);
  const context = plan.agents[0].context;
  assert.deepEqual(Object.keys(context), ["run", "task"]);
  assert.equal(Object.hasOwn(context, "batches"), false);
  assert.ok(plan.agents.every(({ contextBytes }) => contextBytes <= 16_384));
});

test("batch validation produces a deterministic DAG and rejects cycles", () => {
  assert.deepEqual(
    validateBatchGraph([
      { id: "charlie", dependsOn: ["alpha"] },
      { id: "bravo", dependsOn: [] },
      { id: "alpha", dependsOn: [] },
    ]).map(({ id }) => id),
    ["alpha", "bravo", "charlie"],
  );
  assert.throws(
    () => validateBatchGraph([
      { id: "alpha", dependsOn: ["bravo"] },
      { id: "bravo", dependsOn: ["alpha"] },
    ]),
    /contains a cycle/u,
  );
  assert.throws(
    () => validateBatchGraph([{ id: "alpha", dependsOn: ["missing"] }]),
    /unknown batch missing/u,
  );
});

test("concurrent writer scopes must be disjoint, while DAG-ordered reuse is allowed", () => {
  const overlapping = runSpec();
  overlapping.batches = [{
    id: "writers",
    dependsOn: [],
    assignments: [
      assignment({
        id: "tree-writer",
        roleId: "implementer",
        scope: ["sidecar/src/**"],
      }),
      assignment({
        id: "file-writer",
        roleId: "test_writer",
        scope: ["sidecar/src/factory.mjs"],
      }),
    ],
  }];
  assert.throws(
    () => new AgentFactory({ registry }).expand(overlapping),
    /writer scopes overlap without a DAG dependency/u,
  );

  const ordered = runSpec();
  const plan = new AgentFactory({ registry }).expand(ordered);
  const alphaWriters = plan.agents.filter(({ scope }) => scope.includes("sidecar/src/alpha.mjs"));
  assert.equal(alphaWriters.length, 2);
  assert.deepEqual(alphaWriters.map(({ batchId }) => ({ batchId })), [
    { batchId: "writers" },
    { batchId: "tests" },
  ]);
});

test("unsafe paths and unpartitioned multi-writer assignments are rejected", () => {
  const unsafe = runSpec();
  unsafe.batches[0].assignments[0].scopeSets[0] = ["../outside.mjs"];
  assert.throws(
    () => new AgentFactory({ registry }).expand(unsafe),
    /invalid path segment|repository-relative/u,
  );

  const sharedWriter = runSpec();
  sharedWriter.batches = [{
    id: "writers",
    dependsOn: [],
    assignments: [assignment({
      id: "shared",
      roleId: "implementer",
      count: 2,
      scope: ["sidecar/src/factory.mjs"],
    })],
  }];
  assert.throws(
    () => new AgentFactory({ registry }).expand(sharedWriter),
    /one disjoint scopeSets entry per agent/u,
  );
});

test("integrator expands only as one controlled CAS lane", () => {
  const plan = new AgentFactory({ registry }).expand(runSpec());
  const integrator = plan.agents.find(({ roleId }) => roleId === "integrator");
  assert.equal(integrator.executionKind, "controlled-code");
  assert.equal(integrator.archetypeId, "CAS");
  assert.equal(integrator.writeAccess, "integrate");
  assert.deepEqual(integrator.scope, []);

  const invalid = runSpec();
  const integration = invalid.batches.find(({ id }) => id === "integration");
  integration.assignments[0].count = 2;
  assert.throws(
    () => new AgentFactory({ registry }).expand(invalid),
    /one controlled CAS lane/u,
  );

  const duplicateLane = runSpec();
  duplicateLane.batches.push({
    id: "second-integration",
    dependsOn: ["integration"],
    assignments: [assignment({
      id: "second-main-commit-lane",
      roleId: "integrator",
      scope: [],
    })],
  });
  assert.throws(
    () => new AgentFactory({ registry }).expand(duplicateLane),
    /more than one controlled CAS lane/u,
  );
});
