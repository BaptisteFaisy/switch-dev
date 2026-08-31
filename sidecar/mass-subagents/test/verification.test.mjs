import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import path from "node:path";
import test from "node:test";

import {
  VERIFICATION_EVIDENCE_SCHEMA,
  VERIFICATION_LAYER_IDS,
  VERIFICATION_THRESHOLDS,
  createVerificationEvidenceArtifact,
  evaluateVerificationContract,
  serializeVerificationReport,
  verifyVerificationReportHash,
} from "../src/verification.mjs";

const PATCH_SHA256 = "a".repeat(64);
const AUTHOR_AGENT_ID = "agent-implementer";
const REVIEW_DIMENSIONS = [
  "structural_regression",
  "code_judo",
  "branching",
  "boundaries_and_types",
  "file_size",
  "modularity",
  "legibility",
  "orchestration_and_atomicity",
];
const LOW_REVIEW_RISK = {
  sharedBoundaryTouched: false,
  stateOrConcurrencyChanged: false,
  diffLines: 20,
  filesTouched: 2,
  crossesOneThousandLines: false,
  repositoryRequiresSecondReview: false,
};
const NESTED_ARTIFACTS = {};

const nestedArtifact = (label) => {
  const artifact = createVerificationEvidenceArtifact({
    schema: "switch-mass-subagents/test-command-output/v1",
    label,
    exitCode: 0,
  });
  NESTED_ARTIFACTS[artifact.sha256] = artifact.payload;
  return artifact.sha256;
};

const PRECHECK_ARTIFACTS = Object.fromEntries([
  "formatter",
  "linter",
  "static_analysis",
  "file_size",
  "boundaries",
  "atomicity",
  "generated_files",
  "secret_scan",
].map((id) => [id, nestedArtifact(`precheck:${id}`)]));
const AUTHOR_TEST_ARTIFACT = nestedArtifact("author-tests");
const COVERAGE_ARTIFACT = nestedArtifact("targeted-coverage");
const COMMON_REPRO_PARAMETERS = Object.freeze({
  seed: "mass-subagents-seed-42",
  commit_sha: "b".repeat(40),
  config_sha256: "6".repeat(64),
  dependency_lock_sha256: "7".repeat(64),
  toolchain_sha256: "8".repeat(64),
  clean_checkout_each: true,
  cache_cleared_before_each: true,
});

const executionArtifact = ({ kind, id, workerId, outputSha256 }) => {
  const idKey = kind === "replay" ? "run_id" : "build_id";
  const outputKey = kind === "replay" ? "output_sha256" : "image_sha256";
  const artifact = createVerificationEvidenceArtifact({
    schema_version: kind === "replay"
      ? "replay-execution/v1"
      : "reproducible-build-execution/v1",
    [idKey]: id,
    worker_id: workerId,
    patch_sha256: PATCH_SHA256,
    common_parameters: COMMON_REPRO_PARAMETERS,
    [outputKey]: outputSha256,
  });
  NESTED_ARTIFACTS[artifact.sha256] = artifact.payload;
  return artifact.sha256;
};

const REPLAY_OUTPUT_SHA256 = "c".repeat(64);
const BUILD_IMAGE_SHA256 = "d".repeat(64);
const REPLAY_ARTIFACTS = [
  executionArtifact({
    kind: "replay",
    id: "replay-first",
    workerId: "replay-worker-first",
    outputSha256: REPLAY_OUTPUT_SHA256,
  }),
  executionArtifact({
    kind: "replay",
    id: "replay-second",
    workerId: "replay-worker-second",
    outputSha256: REPLAY_OUTPUT_SHA256,
  }),
];
const BUILD_ARTIFACTS = [
  executionArtifact({
    kind: "build",
    id: "build-first",
    workerId: "build-worker-first",
    outputSha256: BUILD_IMAGE_SHA256,
  }),
  executionArtifact({
    kind: "build",
    id: "build-second",
    workerId: "build-worker-second",
    outputSha256: BUILD_IMAGE_SHA256,
  }),
];

const proof = (id, details = {}) => ({
  schema: VERIFICATION_EVIDENCE_SCHEMA,
  id,
  result: "pass",
  patchSha256: PATCH_SHA256,
  ...details,
});

const reviewReport = ({
  reviewerAgentId = "agent-reviewer",
  verdict = "pass",
  secondReviewRequired = false,
  findings = [],
  waivers = [],
} = {}) => ({
  policy: "code-quality-review/v1",
  run_id: "run-review",
  task_id: "task-review",
  attempt_id: "attempt-review",
  review_id: `review-${reviewerAgentId}`,
  reviewer_agent_id: reviewerAgentId,
  author_agent_id: AUTHOR_AGENT_ID,
  base_commit: "b".repeat(40),
  patch_sha256: PATCH_SHA256,
  verdict,
  second_review_required: secondReviewRequired,
  dimensions_checked: [...REVIEW_DIMENSIONS],
  findings,
  waivers,
  created_at: "2026-08-31T00:00:00.000Z",
});

const mutantsMatrix = ({ killed = 19, survived = 1, equivalent = 0, blockerSurvivors = 0 } = {}) => {
  const mutants = [];
  for (let index = 0; index < killed; index += 1) {
    mutants.push({
      mutant_id: `killed-${index + 1}`,
      category: "condition",
      severity: "major",
      status: "killed",
      file: "sidecar/src/example.mjs",
      line: index + 1,
      evidence: "Killed by the deterministic author test suite",
    });
  }
  for (let index = 0; index < survived; index += 1) {
    mutants.push({
      mutant_id: `survived-${index + 1}`,
      category: "boundary",
      severity: index < blockerSurvivors ? "blocker" : "advisory",
      status: "survived",
      file: "sidecar/src/example.mjs",
      line: killed + index + 1,
      evidence: "Survived the deterministic author test suite",
    });
  }
  for (let index = 0; index < equivalent; index += 1) {
    mutants.push({
      mutant_id: `equivalent-${index + 1}`,
      category: "equivalent",
      severity: "advisory",
      status: "equivalent",
      file: "sidecar/src/example.mjs",
      line: killed + survived + index + 1,
      evidence: "Equivalent behavior was proven",
    });
  }
  return {
    schema_version: "mutants-matrix/v1",
    patch_sha256: PATCH_SHA256,
    seed: 42,
    totals: {
      generated: mutants.length,
      killed,
      survived,
      equivalent,
      score: killed + survived === 0 ? 0 : killed / (killed + survived),
    },
    mutants,
  };
};

const precheckReport = () => ({
  schema_version: "precheck-report/v1",
  patch_sha256: PATCH_SHA256,
  checks: Object.entries(PRECHECK_ARTIFACTS).map(([id, artifact_sha256]) => ({
    id,
    status: "pass",
    artifact_sha256,
  })),
  review_risk: { ...LOW_REVIEW_RISK },
});

const authorTestReport = () => ({
  schema_version: "author-test-report/v1",
  patch_sha256: PATCH_SHA256,
  commands: [{
    command: "node --test",
    exit_code: 0,
    artifact_sha256: AUTHOR_TEST_ARTIFACT,
  }],
  summary: { passed: 10, failed: 0, skipped: 0 },
});

const adversarialReport = () => ({
  schema_version: "proof-or-none/v1",
  patch_sha256: PATCH_SHA256,
  author_agent_id: AUTHOR_AGENT_ID,
  adversary_agent_id: "agent-adversary",
  outcome: "no_proof",
  cases_explored: ["concurrent update", "partial append", "invalid boundary"],
  proof: null,
});

const coverageReport = () => ({
  schema_version: "targeted-coverage-report/v1",
  patch_sha256: PATCH_SHA256,
  branch_coverage_pct: 90,
  condition_coverage_pct: 80,
  artifact_sha256: COVERAGE_ARTIFACT,
});

const reproducibilityReport = () => ({
  schema_version: "reproducibility-report/v2",
  patch_sha256: PATCH_SHA256,
  common_parameters: { ...COMMON_REPRO_PARAMETERS },
  replays: [
    {
      run_id: "replay-first",
      worker_id: "replay-worker-first",
      output_sha256: REPLAY_OUTPUT_SHA256,
      artifact_sha256: REPLAY_ARTIFACTS[0],
    },
    {
      run_id: "replay-second",
      worker_id: "replay-worker-second",
      output_sha256: REPLAY_OUTPUT_SHA256,
      artifact_sha256: REPLAY_ARTIFACTS[1],
    },
  ],
  builds: [
    {
      build_id: "build-first",
      worker_id: "build-worker-first",
      image_sha256: BUILD_IMAGE_SHA256,
      artifact_sha256: BUILD_ARTIFACTS[0],
    },
    {
      build_id: "build-second",
      worker_id: "build-worker-second",
      image_sha256: BUILD_IMAGE_SHA256,
      artifact_sha256: BUILD_ARTIFACTS[1],
    },
  ],
});

const passingLayers = () => {
  const layers = {
    L1: proof("L1", { precheckReport: precheckReport() }),
    L2: proof("L2", { testReport: authorTestReport() }),
    L3: proof("L3", { mutantsMatrix: mutantsMatrix() }),
    L4: proof("L4", { adversarialReport: adversarialReport() }),
    L5: proof("L5", { coverageReport: coverageReport() }),
    L6: proof("L6", { reviewReport: reviewReport() }),
    L7: proof("L7", { reproducibilityReport: reproducibilityReport() }),
  };
  const testArtifact = createVerificationEvidenceArtifact(layers.L2).sha256;
  const reviewArtifact = createVerificationEvidenceArtifact(layers.L6).sha256;
  layers.L8 = proof("L8", {
    integrationRecord: {
      schema_version: "integration-record/v1",
      run_id: "run-integration",
      integration_id: "integration-main",
      branch: "refs/heads/main",
      order: 0,
      expected_head: "b".repeat(40),
      candidate_commit: "e".repeat(40),
      patch_sha256: PATCH_SHA256,
      review_report_sha256: reviewArtifact,
      test_artifact_sha256: testArtifact,
      cas_result: "advanced",
      published_commit: "e".repeat(40),
    },
  });
  return layers;
};

const artifactBundle = (layers) => {
  if (layers.L8?.integrationRecord && layers.L2 && layers.L6) {
    layers.L8.integrationRecord.test_artifact_sha256 =
      createVerificationEvidenceArtifact(layers.L2).sha256;
    layers.L8.integrationRecord.review_report_sha256 =
      createVerificationEvidenceArtifact(layers.L6).sha256;
  }
  const references = {};
  const artifacts = { ...NESTED_ARTIFACTS };
  for (const [id, evidence] of Object.entries(layers)) {
    const artifact = createVerificationEvidenceArtifact(evidence);
    references[id] = { id, artifactSha256: artifact.sha256 };
    artifacts[artifact.sha256] = artifact.payload;
  }
  return { references, artifacts };
};

const evaluate = (layers = passingLayers(), authorAgentId = AUTHOR_AGENT_ID) => {
  const { references, artifacts } = artifactBundle(layers);
  return evaluateVerificationContract({
    patchSha256: PATCH_SHA256,
    authorAgentId,
    layers: references,
    artifacts,
  });
};

const resultFor = (report, id) => report.layers.find((layer) => layer.id === id);

test("the cumulative L1-L8 contract passes only with every proof", () => {
  const report = evaluate();
  assert.equal(report.verdict, "pass");
  assert.deepEqual(report.layers.map(({ id }) => id), VERIFICATION_LAYER_IDS);
  assert.deepEqual(report.layers.map(({ result }) => result), Array(8).fill("pass"));
  assert.equal(report.thresholds.mutationScorePctMinimum, 95);
  assert.equal(report.thresholds.branchCoveragePctMinimum, 90);
  assert.equal(report.thresholds.conditionCoveragePctMinimum, 80);
  assert.equal(verifyVerificationReportHash(report), true);
  assert.match(report.reportSha256, /^[a-f0-9]{64}$/u);
  assert.equal(Object.isFrozen(report), true);
  assert.equal(Object.isFrozen(report.layers), true);
  assert.equal(Object.isFrozen(report.layers[0].facts), true);
  assert.throws(() => {
    report.layers[0].result = "fail";
  }, TypeError);
});

test("the adversary role's declared schema accepts the exact L4 report", async () => {
  const rolesRoot = path.resolve(import.meta.dirname, "../agent_roles");
  const manifest = JSON.parse(await readFile(
    path.join(rolesRoot, "adversary", "v1", "manifest.json"),
    "utf8",
  ));
  const schema = JSON.parse(await readFile(
    path.resolve(rolesRoot, ...manifest.out_schema.split("/")),
    "utf8",
  ));
  const example = adversarialReport();

  assert.equal(schema.additionalProperties, false);
  assert.deepEqual(Object.keys(example).sort(), [...schema.required].sort());
  assert.equal(example.schema_version, schema.properties.schema_version.const);
  for (const field of ["patch_sha256", "author_agent_id", "adversary_agent_id"]) {
    assert.match(example[field], new RegExp(schema.properties[field].pattern, "u"));
  }
  assert.ok(schema.properties.outcome.enum.includes(example.outcome));
  assert.ok(example.cases_explored.length >= schema.properties.cases_explored.minItems);
  assert.ok(example.cases_explored.length <= schema.properties.cases_explored.maxItems);
  assert.equal(example.proof, null);
});

test("canonical output and its hash do not depend on input key order", () => {
  const layers = passingLayers();
  const reversed = Object.fromEntries(Object.entries(layers).reverse());
  const first = evaluate(layers);
  const second = evaluate(reversed);
  assert.equal(serializeVerificationReport(first), serializeVerificationReport(second));
  assert.equal(first.reportSha256, second.reportSha256);
});

test("a missing layer produces missing and an uncertain blocking verdict", () => {
  const layers = passingLayers();
  delete layers.L4;
  const report = evaluate(layers);
  assert.equal(report.verdict, "uncertain");
  assert.equal(resultFor(report, "L4").result, "missing");
  assert.deepEqual(resultFor(report, "L4").reasons, ["evidence_missing"]);
});

test("an un-hashed proof is missing evidence and cannot pass", () => {
  const { references, artifacts } = artifactBundle(passingLayers());
  delete references.L2.artifactSha256;
  const report = evaluateVerificationContract({
    patchSha256: PATCH_SHA256,
    authorAgentId: AUTHOR_AGENT_ID,
    layers: references,
    artifacts,
  });
  assert.notEqual(report.verdict, "pass");
  assert.equal(resultFor(report, "L2").result, "missing");
  assert.ok(resultFor(report, "L2").reasons.includes("artifact_sha256_missing"));
});

test("declared hashes cannot pass unless every artifact is resolved and verified", () => {
  const { references, artifacts } = artifactBundle(passingLayers());
  const unresolved = evaluateVerificationContract({
    patchSha256: PATCH_SHA256,
    authorAgentId: AUTHOR_AGENT_ID,
    layers: references,
  });
  assert.equal(unresolved.verdict, "uncertain");
  assert.ok(resultFor(unresolved, "L2").reasons.includes("artifact_not_resolved"));

  const tamperedHash = references.L2.artifactSha256;
  artifacts[tamperedHash] = `${artifacts[tamperedHash]} `;
  const tampered = evaluateVerificationContract({
    patchSha256: PATCH_SHA256,
    authorAgentId: AUTHOR_AGENT_ID,
    layers: references,
    artifacts,
  });
  assert.equal(tampered.verdict, "changes_requested");
  assert.ok(resultFor(tampered, "L2").reasons.includes("artifact_sha256_mismatch"));
});

test("layer facts come from the hashed artifact rather than the reference", () => {
  const layers = passingLayers();
  layers.L1.precheckReport.checks.find(({ id }) => id === "secret_scan").status = "fail";
  const { references, artifacts } = artifactBundle(layers);
  references.L1.secretScanPassed = true;
  const report = evaluateVerificationContract({
    patchSha256: PATCH_SHA256,
    authorAgentId: AUTHOR_AGENT_ID,
    layers: references,
    artifacts,
  });
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L1").reasons.includes("precheck_secret_scan_failed"));
});

test("every proof is bound to the exact patch SHA-256", () => {
  const layers = passingLayers();
  layers.L8.patchSha256 = "b".repeat(64);
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.equal(resultFor(report, "L8").result, "fail");
  assert.ok(resultFor(report, "L8").reasons.includes("patch_sha256_mismatch"));
});

test("L3 blocks a mutation score below 95 percent", () => {
  const layers = passingLayers();
  layers.L3.mutantsMatrix = mutantsMatrix({ killed: 18, survived: 2 });
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.equal(resultFor(report, "L3").result, "fail");
  assert.ok(resultFor(report, "L3").reasons.includes("mutation_score_below_threshold"));
});

test("L3 accepts a score derived from killed and survived counts", () => {
  const layers = passingLayers();
  const report = evaluate(layers);
  assert.equal(report.verdict, "pass");
  assert.equal(resultFor(report, "L3").facts.mutationScorePct, 95);
});

test("L3 blocks every surviving blocker mutant even above the score threshold", () => {
  const layers = passingLayers();
  layers.L3.mutantsMatrix = mutantsMatrix({
    killed: 19,
    survived: 1,
    blockerSurvivors: 1,
  });
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L3").reasons.includes("blocker_mutant_survived"));
});

test("L4 refuses an adversarial report authored by the patch author", () => {
  const layers = passingLayers();
  layers.L4.adversarialReport.adversary_agent_id = AUTHOR_AGENT_ID;
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L4").reasons.includes("adversary_is_patch_author"));
});

test("L4 binds the adversarial report to the declared patch author", () => {
  const layers = passingLayers();
  layers.L4.adversarialReport.author_agent_id = "agent-someone-else";
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L4").reasons.includes("adversarial_author_mismatch"));
});

for (const {
  name,
  field,
  value,
  reason,
} of [
  {
    name: "branch coverage below 90 percent",
    field: "branch_coverage_pct",
    value: 89.99,
    reason: "branch_coverage_below_threshold",
  },
  {
    name: "condition coverage below 80 percent",
    field: "condition_coverage_pct",
    value: 79.99,
    reason: "condition_coverage_below_threshold",
  },
]) {
  test(`L5 blocks ${name}`, () => {
    const layers = passingLayers();
    layers.L5.coverageReport[field] = value;
    const report = evaluate(layers);
    assert.equal(report.verdict, "changes_requested");
    assert.ok(resultFor(report, "L5").reasons.includes(reason));
  });
}

test("L6 requires code-quality-review/v1", () => {
  const layers = passingLayers();
  layers.L6.reviewReport.policy = "code-quality-review/v0";
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L6").reasons.includes("review_policy_mismatch"));
});

test("L6 refuses self-review by the patch author", () => {
  const layers = passingLayers();
  layers.L6.reviewReport.reviewer_agent_id = AUTHOR_AGENT_ID;
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L6").reasons.includes("reviewer_is_patch_author"));
});

test("L6 propagates changes_requested and uncertain review verdicts", async (context) => {
  await context.test("changes_requested is a known failure", () => {
    const layers = passingLayers();
    layers.L6.reviewReport.verdict = "changes_requested";
    const report = evaluate(layers);
    assert.equal(report.verdict, "changes_requested");
    assert.equal(resultFor(report, "L6").result, "fail");
  });
  await context.test("uncertain remains blocked for missing confidence", () => {
    const layers = passingLayers();
    layers.L6.reviewReport.verdict = "uncertain";
    layers.L6.reviewReport.second_review_required = true;
    const report = evaluate(layers);
    assert.equal(report.verdict, "uncertain");
    assert.equal(resultFor(report, "L6").result, "missing");
  });
});

test("L6 enforces a required second independent review", () => {
  const layers = passingLayers();
  layers.L1.precheckReport.review_risk.stateOrConcurrencyChanged = true;
  layers.L6.reviewReport.second_review_required = true;
  const missingSecondReview = evaluate(layers);
  assert.equal(missingSecondReview.verdict, "uncertain");
  assert.equal(resultFor(missingSecondReview, "L6").result, "missing");

  layers.L6.secondReviewReport = reviewReport({
    reviewerAgentId: "agent-reviewer-2",
    secondReviewRequired: true,
  });
  assert.equal(evaluate(layers).verdict, "pass");
});

test("L6 cannot pass with a minimal self-declared wrapper", () => {
  const layers = passingLayers();
  delete layers.L6.reviewReport;
  Object.assign(layers.L6, {
    policy: "code-quality-review/v1",
    verdict: "pass",
    reviewerAgentId: "agent-reviewer",
  });
  const report = evaluate(layers);
  assert.equal(report.verdict, "uncertain");
  assert.ok(resultFor(report, "L6").reasons.includes("review_report_missing"));
});

for (const {
  name,
  collection,
  outputField,
  reason,
} of [
  {
    name: "deterministic replay",
    collection: "replays",
    outputField: "output_sha256",
    reason: "replay_not_reproducible",
  },
  {
    name: "reproducible build",
    collection: "builds",
    outputField: "image_sha256",
    reason: "build_not_reproducible",
  },
]) {
  test(`L7 blocks a non-reproducible ${name}`, () => {
    const layers = passingLayers();
    layers.L7.reproducibilityReport[collection][1][outputField] = "f".repeat(64);
    const report = evaluate(layers);
    assert.equal(report.verdict, "changes_requested");
    assert.ok(resultFor(report, "L7").reasons.includes(reason));
  });
}

test("L7 requires distinct execution artifacts and workers", () => {
  const layers = passingLayers();
  layers.L7.reproducibilityReport.replays[1].artifact_sha256 =
    layers.L7.reproducibilityReport.replays[0].artifact_sha256;
  layers.L7.reproducibilityReport.replays[1].worker_id =
    layers.L7.reproducibilityReport.replays[0].worker_id;
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L7").reasons.includes("reproducibility_report_schema_invalid"));
});

test("L7 binds every execution artifact to the same declared inputs", () => {
  const layers = passingLayers();
  layers.L7.reproducibilityReport.common_parameters.seed = "different-seed";
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L7").reasons.some((reason) =>
    reason.endsWith("_artifact_schema_invalid")));
});

test("L7 requires clean checkouts and cleared caches for both executions", () => {
  const layers = passingLayers();
  layers.L7.reproducibilityReport.common_parameters.cache_cleared_before_each = false;
  const report = evaluate(layers);
  assert.equal(report.verdict, "changes_requested");
  assert.ok(resultFor(report, "L7").reasons.includes("reproducibility_report_schema_invalid"));
});

test("L1 blocks a failed or missing secret scan", async (context) => {
  await context.test("failed scan", () => {
    const layers = passingLayers();
    layers.L1.precheckReport.checks.find(({ id }) => id === "secret_scan").status = "fail";
    const report = evaluate(layers);
    assert.equal(report.verdict, "changes_requested");
    assert.ok(resultFor(report, "L1").reasons.includes("precheck_secret_scan_failed"));
  });
  await context.test("missing scan proof", () => {
    const layers = passingLayers();
    const { references, artifacts } = artifactBundle(layers);
    delete artifacts[PRECHECK_ARTIFACTS.secret_scan];
    const report = evaluateVerificationContract({
      patchSha256: PATCH_SHA256,
      authorAgentId: AUTHOR_AGENT_ID,
      layers: references,
      artifacts,
    });
    assert.equal(report.verdict, "uncertain");
    assert.equal(resultFor(report, "L1").result, "missing");
    assert.ok(resultFor(report, "L1").reasons.includes(
      "precheck_secret_scan_artifact_not_resolved",
    ));
  });
});

for (const [id, field, reason] of [
  ["L2", "testReport", "author_test_report_missing"],
  ["L4", "adversarialReport", "adversarial_report_missing"],
  ["L8", "integrationRecord", "integration_record_missing"],
]) {
  test(`${id} cannot pass with a bare declarative artifact`, () => {
    const layers = passingLayers();
    delete layers[id][field];
    const report = evaluate(layers);
    assert.equal(report.verdict, "uncertain");
    assert.ok(resultFor(report, id).reasons.includes(reason));
  });
}

test("the contract cannot be created without a valid patch SHA-256", () => {
  assert.throws(
    () => evaluateVerificationContract({ patchSha256: "not-a-hash", layers: passingLayers() }),
    /patchSha256/u,
  );
});
