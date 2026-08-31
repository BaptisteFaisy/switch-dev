import {
  canonicalJson,
  deepFreeze,
  sha256,
} from "./canonical.mjs";

const SHA256 = /^[a-f0-9]{64}$/u;
const IDENTIFIER = /^[A-Za-z0-9][A-Za-z0-9_-]{0,127}$/u;
const COMMIT = /^[a-f0-9]{7,64}$/u;
const LAYER_RESULTS = new Set(["pass", "fail", "missing"]);
const REVIEW_DIMENSIONS = Object.freeze([
  "structural_regression",
  "code_judo",
  "branching",
  "boundaries_and_types",
  "file_size",
  "modularity",
  "legibility",
  "orchestration_and_atomicity",
]);
const REVIEW_REPORT_KEYS = Object.freeze([
  "policy",
  "run_id",
  "task_id",
  "attempt_id",
  "review_id",
  "reviewer_agent_id",
  "author_agent_id",
  "base_commit",
  "patch_sha256",
  "verdict",
  "second_review_required",
  "dimensions_checked",
  "findings",
  "waivers",
  "created_at",
]);
const REVIEW_RISK_KEYS = Object.freeze([
  "sharedBoundaryTouched",
  "stateOrConcurrencyChanged",
  "diffLines",
  "filesTouched",
  "crossesOneThousandLines",
  "repositoryRequiresSecondReview",
]);
const PRECHECK_IDS = Object.freeze([
  "formatter",
  "linter",
  "static_analysis",
  "file_size",
  "boundaries",
  "atomicity",
  "generated_files",
  "secret_scan",
]);

export const VERIFICATION_SCHEMA = "switch-mass-subagents/cumulative-verification/v1";
export const VERIFICATION_EVIDENCE_SCHEMA = "switch-mass-subagents/layer-evidence/v1";

export const VERIFICATION_THRESHOLDS = deepFreeze({
  mutationScorePctMinimum: 95,
  blockerMutantSurvivorsMaximum: 0,
  branchCoveragePctMinimum: 90,
  conditionCoveragePctMinimum: 80,
});

export const VERIFICATION_LAYERS = deepFreeze([
  { id: "L1", name: "deterministic_prechecks_and_secret_scan" },
  { id: "L2", name: "author_tests" },
  { id: "L3", name: "mutation_testing" },
  { id: "L4", name: "adversarial_tests" },
  { id: "L5", name: "targeted_coverage" },
  { id: "L6", name: "structural_review" },
  { id: "L7", name: "deterministic_replay_and_reproducible_build" },
  { id: "L8", name: "verified_integration" },
]);

export const VERIFICATION_LAYER_IDS = Object.freeze(
  VERIFICATION_LAYERS.map(({ id }) => id),
);

const isSha256 = (value) => typeof value === "string" && SHA256.test(value);
const isNonEmptyString = (value) => typeof value === "string" && value.trim().length > 0;
const hasOwn = (value, key) => Object.prototype.hasOwnProperty.call(value, key);

const field = (proof, names) => {
  const containers = [proof, proof?.details, proof?.metrics];
  for (const container of containers) {
    if (!container || typeof container !== "object" || Array.isArray(container)) continue;
    for (const name of names) {
      if (hasOwn(container, name)) return container[name];
    }
  }
  return undefined;
};

const normalizePercent = (value) => {
  if (typeof value !== "number" || !Number.isFinite(value)) return null;
  return Number(value.toFixed(4));
};

const proofIndex = (layers) => {
  const indexed = new Map();
  const duplicates = new Set();
  if (Array.isArray(layers)) {
    for (const proof of layers) {
      const id = proof?.id;
      if (!VERIFICATION_LAYER_IDS.includes(id)) continue;
      if (indexed.has(id)) duplicates.add(id);
      else indexed.set(id, proof);
    }
    return { indexed, duplicates };
  }
  if (layers && typeof layers === "object") {
    for (const id of VERIFICATION_LAYER_IDS) {
      if (hasOwn(layers, id)) indexed.set(id, layers[id]);
    }
  }
  return { indexed, duplicates };
};

const artifactValue = (artifacts, digest) => {
  if (artifacts instanceof Map) return artifacts.get(digest);
  if (artifacts && typeof artifacts === "object" && hasOwn(artifacts, digest)) {
    return artifacts[digest];
  }
  return undefined;
};

const parseEvidenceArtifact = (value) => {
  if (Buffer.isBuffer(value)) return JSON.parse(value.toString("utf8"));
  if (typeof value === "string") return JSON.parse(value);
  if (value && typeof value === "object" && !Array.isArray(value)) return value;
  throw new TypeError("evidence artifact must be a JSON object or JSON bytes");
};

const nestedArtifactVerified = (state, digest, artifacts, prefix) => {
  if (!isSha256(digest)) {
    state.fail(`${prefix}_artifact_sha256_invalid`);
    return false;
  }
  const payload = artifactValue(artifacts, digest);
  if (payload === undefined) {
    state.miss(`${prefix}_artifact_not_resolved`);
    return false;
  }
  if (sha256(payload) !== digest) {
    state.fail(`${prefix}_artifact_sha256_mismatch`);
    return false;
  }
  return true;
};

const createLayerState = (
  definition,
  reference,
  expectedPatchSha256,
  duplicate,
  artifacts,
) => {
  const failures = [];
  const missing = [];
  const facts = {};
  const fail = (reason) => failures.push(reason);
  const miss = (reason) => missing.push(reason);

  if (duplicate) fail("duplicate_layer_evidence");
  if (reference === undefined || reference === null) {
    miss("evidence_missing");
    return { definition, proof: null, failures, missing, facts, fail, miss };
  }
  if (typeof reference !== "object" || Array.isArray(reference)) {
    fail("evidence_invalid");
    return { definition, proof: null, failures, missing, facts, fail, miss };
  }

  if (reference.id !== undefined && reference.id !== definition.id) fail("layer_id_mismatch");

  const artifactSha256 = field(reference, [
    "artifactSha256",
    "artifact_sha256",
    "evidenceSha256",
    "evidence_sha256",
    "reportSha256",
    "report_sha256",
  ]) ?? reference.artifact?.sha256;
  if (artifactSha256 === undefined) miss("artifact_sha256_missing");
  else if (!isSha256(artifactSha256)) fail("artifact_sha256_invalid");
  else {
    facts.artifactSha256 = artifactSha256;
    const rawArtifact = artifactValue(artifacts, artifactSha256);
    if (rawArtifact === undefined) {
      miss("artifact_not_resolved");
    } else if (sha256(rawArtifact) !== artifactSha256) {
      fail("artifact_sha256_mismatch");
    } else {
      let proof;
      try {
        proof = parseEvidenceArtifact(rawArtifact);
      } catch {
        fail("artifact_json_invalid");
      }
      if (proof) {
        if (proof.schema !== VERIFICATION_EVIDENCE_SCHEMA) fail("artifact_schema_mismatch");
        if (proof.id !== definition.id) fail("artifact_layer_id_mismatch");

        const reportedResult = field(proof, ["result", "status"]);
        if (reportedResult === undefined) miss("result_missing");
        else if (!LAYER_RESULTS.has(reportedResult)) fail("result_invalid");
        else if (reportedResult === "fail") fail("evidence_reported_failure");
        else if (reportedResult === "missing") miss("evidence_reported_missing");

        const linkedPatchSha256 = field(proof, ["patchSha256", "patch_sha256"]);
        if (linkedPatchSha256 === undefined) miss("patch_sha256_missing");
        else if (!isSha256(linkedPatchSha256)) fail("patch_sha256_invalid");
        else if (linkedPatchSha256 !== expectedPatchSha256) fail("patch_sha256_mismatch");
        else facts.patchSha256 = linkedPatchSha256;

        return { definition, proof, failures, missing, facts, fail, miss };
      }
    }
  }

  return { definition, proof: null, failures, missing, facts, fail, miss };
};

const hasExactKeys = (value, keys) => value !== null
  && typeof value === "object"
  && !Array.isArray(value)
  && Object.keys(value).length === keys.length
  && keys.every((key) => hasOwn(value, key));

const isBoundedString = (value, minimum, maximum) => typeof value === "string"
  && value.length >= minimum
  && value.length <= maximum;

const uniqueStringArray = (value, minimum, maximum, itemMaximum) => Array.isArray(value)
  && value.length >= minimum
  && value.length <= maximum
  && new Set(value).size === value.length
  && value.every((item) => isBoundedString(item, 1, itemMaximum));

const evaluatePrechecks = (state, _authorAgentId, { artifacts } = {}) => {
  const report = state.proof.precheckReport;
  if (report === undefined) {
    state.miss("precheck_report_missing");
    return;
  }
  const validReport = hasExactKeys(report, [
    "schema_version",
    "patch_sha256",
    "checks",
    "review_risk",
  ])
    && report.schema_version === "precheck-report/v1"
    && report.patch_sha256 === state.facts.patchSha256
    && Array.isArray(report.checks)
    && report.checks.length === PRECHECK_IDS.length
    && new Set(report.checks.map(({ id }) => id)).size === PRECHECK_IDS.length
    && PRECHECK_IDS.every((id) => report.checks.some((check) => check?.id === id))
    && report.checks.every((check) => hasExactKeys(check, ["id", "status", "artifact_sha256"])
      && PRECHECK_IDS.includes(check.id)
      && ["pass", "fail"].includes(check.status));
  if (!validReport) {
    state.fail("precheck_report_schema_invalid");
    return;
  }
  for (const check of report.checks) {
    nestedArtifactVerified(state, check.artifact_sha256, artifacts, `precheck_${check.id}`);
    if (check.status !== "pass") state.fail(`precheck_${check.id}_failed`);
  }
  state.facts.secretScanPassed = report.checks
    .find(({ id }) => id === "secret_scan")?.status === "pass";

  const risk = report.review_risk;
  if (risk === undefined) {
    state.miss("review_risk_missing");
    state.facts.reviewRisk = null;
    return;
  }
  const validRisk = hasExactKeys(risk, REVIEW_RISK_KEYS)
    && [
      "sharedBoundaryTouched",
      "stateOrConcurrencyChanged",
      "crossesOneThousandLines",
      "repositoryRequiresSecondReview",
    ].every((key) => typeof risk[key] === "boolean")
    && Number.isSafeInteger(risk.diffLines)
    && risk.diffLines >= 0
    && Number.isSafeInteger(risk.filesTouched)
    && risk.filesTouched >= 0;
  if (!validRisk) state.fail("review_risk_invalid");
  state.facts.reviewRisk = validRisk ? { ...risk } : null;
};

const evaluateMutation = (state) => {
  const matrix = state.proof.mutantsMatrix;
  if (matrix === undefined) {
    state.miss("mutants_matrix_missing");
    return;
  }
  const matrixKeys = ["schema_version", "patch_sha256", "seed", "totals", "mutants"];
  const totalKeys = ["generated", "killed", "survived", "equivalent", "score"];
  const mutantKeys = ["mutant_id", "category", "severity", "status", "file", "line", "evidence"];
  let schemaValid = hasExactKeys(matrix, matrixKeys)
    && matrix.schema_version === "mutants-matrix/v1"
    && matrix.patch_sha256 === state.facts.patchSha256
    && Number.isSafeInteger(matrix.seed)
    && matrix.seed >= 0
    && hasExactKeys(matrix.totals, totalKeys)
    && Array.isArray(matrix.mutants)
    && matrix.mutants.length <= 100_000;
  const seenIds = new Set();
  if (schemaValid) {
    for (const mutant of matrix.mutants) {
      if (!hasExactKeys(mutant, mutantKeys)
        || typeof mutant.mutant_id !== "string"
        || !IDENTIFIER.test(mutant.mutant_id)
        || seenIds.has(mutant.mutant_id)
        || !isBoundedString(mutant.category, 1, 128)
        || !["blocker", "major", "advisory"].includes(mutant.severity)
        || !["killed", "survived", "equivalent"].includes(mutant.status)
        || !isBoundedString(mutant.file, 1, 512)
        || !Number.isInteger(mutant.line)
        || mutant.line < 1
        || !isBoundedString(mutant.evidence, 1, 4_096)) {
        schemaValid = false;
        break;
      }
      seenIds.add(mutant.mutant_id);
    }
  }
  if (!schemaValid) {
    state.fail("mutants_matrix_schema_invalid");
    return;
  }

  const killed = matrix.mutants.filter(({ status }) => status === "killed").length;
  const survived = matrix.mutants.filter(({ status }) => status === "survived").length;
  const equivalent = matrix.mutants.filter(({ status }) => status === "equivalent").length;
  const score = killed + survived > 0 ? (killed / (killed + survived)) * 100 : null;
  const declaredScore = score === null ? 0 : score / 100;
  if (matrix.totals.generated !== matrix.mutants.length
    || matrix.totals.killed !== killed
    || matrix.totals.survived !== survived
    || matrix.totals.equivalent !== equivalent
    || typeof matrix.totals.score !== "number"
    || Math.abs(matrix.totals.score - declaredScore) > 0.000001) {
    state.fail("mutants_matrix_totals_mismatch");
  }
  const normalizedScore = normalizePercent(score);
  if (score === null) state.miss("mutation_score_missing");
  else if (score < VERIFICATION_THRESHOLDS.mutationScorePctMinimum) {
    state.fail("mutation_score_below_threshold");
  }
  state.facts.mutationScorePct = normalizedScore;

  const blockerSurvivors = matrix.mutants.filter((mutant) =>
    mutant.status === "survived" && mutant.severity === "blocker").length;
  if (blockerSurvivors > VERIFICATION_THRESHOLDS.blockerMutantSurvivorsMaximum) {
    state.fail("blocker_mutant_survived");
  }
  state.facts.blockerSurvivors = blockerSurvivors;
  state.facts.mutationSeed = matrix.seed;
};

const evaluateAuthorTests = (state, _authorAgentId, { artifacts } = {}) => {
  const report = state.proof.testReport;
  if (report === undefined) {
    state.miss("author_test_report_missing");
    return;
  }
  const valid = hasExactKeys(report, [
    "schema_version",
    "patch_sha256",
    "commands",
    "summary",
  ])
    && report.schema_version === "author-test-report/v1"
    && report.patch_sha256 === state.facts.patchSha256
    && Array.isArray(report.commands)
    && report.commands.length >= 1
    && report.commands.length <= 64
    && report.commands.every((command) => hasExactKeys(
      command,
      ["command", "exit_code", "artifact_sha256"],
    )
      && isBoundedString(command.command, 1, 1_024)
      && Number.isInteger(command.exit_code))
    && hasExactKeys(report.summary, ["passed", "failed", "skipped"])
    && ["passed", "failed", "skipped"].every((key) =>
      Number.isSafeInteger(report.summary[key]) && report.summary[key] >= 0);
  if (!valid) {
    state.fail("author_test_report_schema_invalid");
    return;
  }
  for (const [index, command] of report.commands.entries()) {
    nestedArtifactVerified(state, command.artifact_sha256, artifacts, `author_test_${index}`);
    if (command.exit_code !== 0) state.fail("author_test_command_failed");
  }
  if (report.summary.passed < 1) state.miss("author_tests_empty");
  if (report.summary.failed > 0) state.fail("author_tests_failed");
  state.facts.authorTestsPassed = report.summary.passed;
};

const coveragePercent = (state, names, threshold, label) => {
  const value = field(state.proof, names);
  const normalized = normalizePercent(value);
  if (value === undefined) state.miss(`${label}_coverage_missing`);
  else if (normalized === null || value < 0 || value > 100) {
    state.fail(`${label}_coverage_invalid`);
  } else if (value < threshold) state.fail(`${label}_coverage_below_threshold`);
  return normalized;
};

const evaluateCoverage = (state, _authorAgentId, { artifacts } = {}) => {
  const report = state.proof.coverageReport;
  if (report === undefined) {
    state.miss("coverage_report_missing");
    return;
  }
  if (!hasExactKeys(report, [
    "schema_version",
    "patch_sha256",
    "branch_coverage_pct",
    "condition_coverage_pct",
    "artifact_sha256",
  ])
    || report.schema_version !== "targeted-coverage-report/v1"
    || report.patch_sha256 !== state.facts.patchSha256) {
    state.fail("coverage_report_schema_invalid");
    return;
  }
  nestedArtifactVerified(state, report.artifact_sha256, artifacts, "coverage");
  state.facts.branchCoveragePct = coveragePercent(
    { ...state, proof: report },
    ["branch_coverage_pct"],
    VERIFICATION_THRESHOLDS.branchCoveragePctMinimum,
    "branch",
  );
  state.facts.conditionCoveragePct = coveragePercent(
    { ...state, proof: report },
    ["condition_coverage_pct"],
    VERIFICATION_THRESHOLDS.conditionCoveragePctMinimum,
    "condition",
  );
};

const evaluateAdversarialProof = (state, authorAgentId) => {
  const report = state.proof.adversarialReport;
  if (report === undefined) {
    state.miss("adversarial_report_missing");
    return;
  }
  const proofKeys = ["category", "file", "line", "reproduction", "expected", "observed"];
  const proofValid = report?.proof === null || (
    hasExactKeys(report?.proof, proofKeys)
    && isBoundedString(report.proof.category, 1, 128)
    && isBoundedString(report.proof.file, 1, 512)
    && (report.proof.line === null
      || (Number.isInteger(report.proof.line) && report.proof.line >= 1))
    && isBoundedString(report.proof.reproduction, 1, 8_192)
    && isBoundedString(report.proof.expected, 1, 4_096)
    && isBoundedString(report.proof.observed, 1, 4_096)
  );
  const valid = hasExactKeys(
    report,
    [
      "schema_version",
      "patch_sha256",
      "author_agent_id",
      "adversary_agent_id",
      "outcome",
      "cases_explored",
      "proof",
    ],
  )
    && report.schema_version === "proof-or-none/v1"
    && report.patch_sha256 === state.facts.patchSha256
    && IDENTIFIER.test(report.author_agent_id ?? "")
    && IDENTIFIER.test(report.adversary_agent_id ?? "")
    && ["bug_proven", "no_proof"].includes(report.outcome)
    && uniqueStringArray(report.cases_explored, 1, 256, 2_048)
    && proofValid
    && ((report.outcome === "bug_proven") === (report.proof !== null));
  if (!valid) {
    state.fail("adversarial_report_schema_invalid");
    return;
  }
  if (report.author_agent_id !== authorAgentId) state.fail("adversarial_author_mismatch");
  if (report.adversary_agent_id === authorAgentId) state.fail("adversary_is_patch_author");
  if (report.outcome === "bug_proven") state.fail("adversarial_bug_proven");
  state.facts.adversaryAgentId = report.adversary_agent_id;
  state.facts.adversarialCasesExplored = report.cases_explored.length;
};

const evaluateReviewVerdict = (state, verdict, prefix = "review") => {
  if (verdict === undefined) state.miss(`${prefix}_verdict_missing`);
  else if (verdict === "changes_requested") state.fail(`${prefix}_changes_requested`);
  else if (verdict === "uncertain") state.miss(`${prefix}_uncertain`);
  else if (verdict !== "pass") state.fail(`${prefix}_verdict_invalid`);
};

const validReviewFinding = (finding) => {
  const keys = [
    "finding_id",
    "severity",
    "category",
    "confidence",
    "file",
    "line",
    "evidence",
    "impact",
    "recommended_change",
    "complexity_removed",
    "verification",
  ];
  return hasExactKeys(finding, keys)
    && IDENTIFIER.test(finding.finding_id ?? "")
    && ["blocker", "major", "advisory"].includes(finding.severity)
    && isBoundedString(finding.category, 1, 128)
    && typeof finding.confidence === "number"
    && Number.isFinite(finding.confidence)
    && finding.confidence >= 0
    && finding.confidence <= 1
    && (finding.file === null || isBoundedString(finding.file, 1, 512))
    && (finding.line === null || (Number.isInteger(finding.line) && finding.line >= 1))
    && isBoundedString(finding.evidence, 1, 4_096)
    && isBoundedString(finding.impact, 1, 4_096)
    && isBoundedString(finding.recommended_change, 1, 4_096)
    && uniqueStringArray(finding.complexity_removed, 0, 32, 512)
    && uniqueStringArray(finding.verification, 1, 32, 1_024);
};

const validReviewWaiver = (waiver) => {
  const keys = ["finding_id", "author", "justification", "evidence", "scope", "expires_at"];
  return hasExactKeys(waiver, keys)
    && IDENTIFIER.test(waiver.finding_id ?? "")
    && IDENTIFIER.test(waiver.author ?? "")
    && isBoundedString(waiver.justification, 1, 4_096)
    && isBoundedString(waiver.evidence, 1, 4_096)
    && isBoundedString(waiver.scope, 1, 1_024)
    && (waiver.expires_at === null
      || (typeof waiver.expires_at === "string"
        && Number.isFinite(Date.parse(waiver.expires_at))));
};

const validateReviewReport = (
  state,
  report,
  {
    prefix,
    authorAgentId,
    expectedSecondReviewRequired,
    otherReviewerAgentId = null,
  },
) => {
  const findings = report?.findings;
  const waivers = report?.waivers;
  const schemaValid = hasExactKeys(report, REVIEW_REPORT_KEYS)
    && typeof report.policy === "string"
    && ["run_id", "task_id", "attempt_id", "review_id", "reviewer_agent_id", "author_agent_id"]
      .every((key) => IDENTIFIER.test(report[key] ?? ""))
    && COMMIT.test(report.base_commit ?? "")
    && isSha256(report.patch_sha256)
    && ["pass", "changes_requested", "uncertain"].includes(report.verdict)
    && typeof report.second_review_required === "boolean"
    && Array.isArray(report.dimensions_checked)
    && report.dimensions_checked.length === REVIEW_DIMENSIONS.length
    && new Set(report.dimensions_checked).size === REVIEW_DIMENSIONS.length
    && REVIEW_DIMENSIONS.every((dimension) => report.dimensions_checked.includes(dimension))
    && Array.isArray(findings)
    && findings.length <= 7
    && findings.every(validReviewFinding)
    && new Set(findings?.map(({ finding_id: id }) => id)).size === findings?.length
    && Array.isArray(waivers)
    && waivers.length <= 7
    && waivers.every(validReviewWaiver)
    && new Set(waivers?.map(({ finding_id: id }) => id)).size === waivers?.length
    && typeof report.created_at === "string"
    && Number.isFinite(Date.parse(report.created_at));
  if (!schemaValid) {
    state.fail(`${prefix}_report_schema_invalid`);
    return null;
  }
  if (report.policy !== "code-quality-review/v1") state.fail("review_policy_mismatch");
  if (report.patch_sha256 !== state.facts.patchSha256) {
    state.fail(`${prefix}_patch_sha256_mismatch`);
  }
  if (report.author_agent_id !== authorAgentId) state.fail(`${prefix}_author_mismatch`);
  if (report.reviewer_agent_id === authorAgentId) {
    state.fail(prefix === "review" ? "reviewer_is_patch_author" : "second_reviewer_not_independent");
  }
  if (otherReviewerAgentId && report.reviewer_agent_id === otherReviewerAgentId) {
    state.fail("second_reviewer_not_independent");
  }
  if (report.second_review_required !== expectedSecondReviewRequired) {
    state.fail(`${prefix}_second_review_requirement_mismatch`);
  }
  const findingIds = new Set(findings.map(({ finding_id: id }) => id));
  if (waivers.some(({ finding_id: id }) => !findingIds.has(id))) {
    state.fail(`${prefix}_orphan_waiver`);
  }
  const waived = new Set(waivers.map(({ finding_id: id }) => id));
  if (findings.some((finding) => ["blocker", "major"].includes(finding.severity)
    && !waived.has(finding.finding_id))) {
    state.fail("blocking_review_finding");
  }
  evaluateReviewVerdict(state, report.verdict, prefix);
  return report;
};

const evaluateReview = (state, authorAgentId, { statesById } = {}) => {
  if (!isNonEmptyString(authorAgentId) || !IDENTIFIER.test(authorAgentId)) {
    state.miss("author_agent_id_missing");
  }
  const report = state.proof.reviewReport;
  if (report === undefined) {
    state.miss("review_report_missing");
    return;
  }
  const risk = statesById?.get("L1")?.facts.reviewRisk ?? null;
  if (!risk) state.miss("review_risk_missing");
  const reportHasBlocker = Array.isArray(report.findings)
    && report.findings.some(({ severity }) => severity === "blocker");
  const derivedSecondReviewRequired = Boolean(risk && (
    risk.sharedBoundaryTouched
    || risk.stateOrConcurrencyChanged
    || risk.diffLines > 800
    || risk.filesTouched > 20
    || risk.crossesOneThousandLines
    || risk.repositoryRequiresSecondReview
    || report.verdict === "uncertain"
    || reportHasBlocker
  ));
  const validated = validateReviewReport(state, report, {
    prefix: "review",
    authorAgentId,
    expectedSecondReviewRequired: derivedSecondReviewRequired,
  });
  state.facts.policy = validated?.policy ?? null;
  state.facts.reviewerAgentId = validated?.reviewer_agent_id ?? null;
  state.facts.secondReviewRequired = derivedSecondReviewRequired;
  if (!validated || !derivedSecondReviewRequired) return;

  const second = state.proof.secondReviewReport;
  if (second === undefined) {
    state.miss("second_review_report_missing");
    return;
  }
  const secondValidated = validateReviewReport(state, second, {
    prefix: "second_review",
    authorAgentId,
    expectedSecondReviewRequired: true,
    otherReviewerAgentId: validated.reviewer_agent_id,
  });
  state.facts.secondReviewerAgentId = secondValidated?.reviewer_agent_id ?? null;
};

const evaluateReproducibility = (state, _authorAgentId, { artifacts } = {}) => {
  const report = state.proof.reproducibilityReport;
  if (report === undefined) {
    state.miss("reproducibility_report_missing");
    return;
  }
  const itemValid = (item, idKey, outputKey) => hasExactKeys(
    item,
    [idKey, "worker_id", outputKey, "artifact_sha256"],
  )
    && IDENTIFIER.test(item[idKey] ?? "")
    && IDENTIFIER.test(item.worker_id ?? "")
    && isSha256(item[outputKey])
    && isSha256(item.artifact_sha256);
  const common = report?.common_parameters;
  const commonValid = hasExactKeys(common, [
    "seed",
    "commit_sha",
    "config_sha256",
    "dependency_lock_sha256",
    "toolchain_sha256",
    "clean_checkout_each",
    "cache_cleared_before_each",
  ])
    && isBoundedString(common.seed, 1, 256)
    && COMMIT.test(common.commit_sha ?? "")
    && isSha256(common.config_sha256)
    && isSha256(common.dependency_lock_sha256)
    && isSha256(common.toolchain_sha256)
    && common.clean_checkout_each === true
    && common.cache_cleared_before_each === true;
  const valid = hasExactKeys(
    report,
    ["schema_version", "patch_sha256", "common_parameters", "replays", "builds"],
  )
    && report.schema_version === "reproducibility-report/v2"
    && report.patch_sha256 === state.facts.patchSha256
    && commonValid
    && Array.isArray(report.replays)
    && report.replays.length === 2
    && report.replays.every((item) => itemValid(item, "run_id", "output_sha256"))
    && report.replays[0].run_id !== report.replays[1].run_id
    && report.replays[0].worker_id !== report.replays[1].worker_id
    && report.replays[0].artifact_sha256 !== report.replays[1].artifact_sha256
    && Array.isArray(report.builds)
    && report.builds.length === 2
    && report.builds.every((item) => itemValid(item, "build_id", "image_sha256"))
    && report.builds[0].build_id !== report.builds[1].build_id
    && report.builds[0].worker_id !== report.builds[1].worker_id
    && report.builds[0].artifact_sha256 !== report.builds[1].artifact_sha256;
  if (!valid) {
    state.fail("reproducibility_report_schema_invalid");
    return;
  }
  const validateExecutionArtifact = (item, kind, idKey, outputKey) => {
    const prefix = `${kind}_${item[idKey]}`;
    if (!nestedArtifactVerified(state, item.artifact_sha256, artifacts, prefix)) return;
    let artifact;
    try {
      artifact = parseEvidenceArtifact(artifactValue(artifacts, item.artifact_sha256));
    } catch {
      state.fail(`${prefix}_artifact_json_invalid`);
      return;
    }
    const schemaVersion = kind === "replay"
      ? "replay-execution/v1"
      : "reproducible-build-execution/v1";
    const artifactValid = hasExactKeys(artifact, [
      "schema_version",
      idKey,
      "worker_id",
      "patch_sha256",
      "common_parameters",
      outputKey,
    ])
      && artifact.schema_version === schemaVersion
      && artifact[idKey] === item[idKey]
      && artifact.worker_id === item.worker_id
      && artifact.patch_sha256 === state.facts.patchSha256
      && canonicalJson(artifact.common_parameters) === canonicalJson(common)
      && artifact[outputKey] === item[outputKey];
    if (!artifactValid) state.fail(`${prefix}_artifact_schema_invalid`);
  };
  for (const replay of report.replays) {
    validateExecutionArtifact(replay, "replay", "run_id", "output_sha256");
  }
  for (const build of report.builds) {
    validateExecutionArtifact(build, "build", "build_id", "image_sha256");
  }
  const replayReproducible = report.replays[0].output_sha256
    === report.replays[1].output_sha256;
  const buildReproducible = report.builds[0].image_sha256
    === report.builds[1].image_sha256;
  if (!replayReproducible) state.fail("replay_not_reproducible");
  if (!buildReproducible) state.fail("build_not_reproducible");
  state.facts.replayReproducible = replayReproducible;
  state.facts.buildReproducible = buildReproducible;
};

const evaluateIntegration = (state, _authorAgentId, { statesById } = {}) => {
  const record = state.proof.integrationRecord;
  if (record === undefined) {
    state.miss("integration_record_missing");
    return;
  }
  const valid = hasExactKeys(record, [
    "schema_version",
    "run_id",
    "integration_id",
    "branch",
    "order",
    "expected_head",
    "candidate_commit",
    "patch_sha256",
    "review_report_sha256",
    "test_artifact_sha256",
    "cas_result",
    "published_commit",
  ])
    && record.schema_version === "integration-record/v1"
    && IDENTIFIER.test(record.run_id ?? "")
    && IDENTIFIER.test(record.integration_id ?? "")
    && /^refs\/heads\/[A-Za-z0-9._/-]{1,240}$/u.test(record.branch ?? "")
    && Number.isSafeInteger(record.order)
    && record.order >= 0
    && COMMIT.test(record.expected_head ?? "")
    && COMMIT.test(record.candidate_commit ?? "")
    && record.patch_sha256 === state.facts.patchSha256
    && isSha256(record.review_report_sha256)
    && isSha256(record.test_artifact_sha256)
    && ["advanced", "stale_head", "rejected"].includes(record.cas_result)
    && (record.published_commit === null || COMMIT.test(record.published_commit ?? ""))
    && ((record.cas_result === "advanced") === (record.published_commit !== null));
  if (!valid) {
    state.fail("integration_record_schema_invalid");
    return;
  }
  if (record.review_report_sha256 !== statesById?.get("L6")?.facts.artifactSha256) {
    state.fail("integration_review_artifact_mismatch");
  }
  if (record.test_artifact_sha256 !== statesById?.get("L2")?.facts.artifactSha256) {
    state.fail("integration_test_artifact_mismatch");
  }
  if (record.cas_result !== "advanced" || record.published_commit !== record.candidate_commit) {
    state.fail("integration_not_advanced");
  }
  state.facts.publishedCommit = record.published_commit;
};

const specializedEvaluators = Object.freeze({
  L1: evaluatePrechecks,
  L2: evaluateAuthorTests,
  L3: evaluateMutation,
  L4: evaluateAdversarialProof,
  L5: evaluateCoverage,
  L6: evaluateReview,
  L7: evaluateReproducibility,
  L8: evaluateIntegration,
});

const finishLayer = (state) => {
  const result = state.failures.length > 0
    ? "fail"
    : state.missing.length > 0
      ? "missing"
      : "pass";
  return {
    id: state.definition.id,
    name: state.definition.name,
    result,
    artifactSha256: state.facts.artifactSha256 ?? null,
    facts: state.facts,
    reasons: [...state.failures, ...state.missing],
  };
};

const verdictFor = (layers) => {
  if (layers.some(({ result }) => result === "fail")) return "changes_requested";
  if (layers.some(({ result }) => result === "missing")) return "uncertain";
  return "pass";
};

/**
 * Évalue les huit preuves cumulatives sans exécuter de commande ni accéder au
 * réseau. `layers` référence un artefact par hash pour chaque L1..L8 et
 * `artifacts` fournit les octets correspondants. Les résultats et métriques
 * sont lus dans ces artefacts vérifiés, jamais dans la référence déclarative.
 */
export const evaluateVerificationContract = ({
  patchSha256,
  authorAgentId,
  layers,
  artifacts,
} = {}) => {
  if (!isSha256(patchSha256)) {
    throw new TypeError("patchSha256 must be a lowercase SHA-256 digest");
  }
  const { indexed, duplicates } = proofIndex(layers);
  const states = VERIFICATION_LAYERS.map((definition) => createLayerState(
      definition,
      indexed.get(definition.id),
      patchSha256,
      duplicates.has(definition.id),
      artifacts,
    ));
  const statesById = new Map(states.map((state) => [state.definition.id, state]));
  for (const state of states) {
    const definition = state.definition;
    const evaluate = specializedEvaluators[definition.id];
    if (evaluate && state.proof) evaluate(state, authorAgentId, { statesById, artifacts });
  }
  const evaluatedLayers = states.map(finishLayer);
  const body = {
    kind: "cumulative_verification",
    version: 1,
    schema: VERIFICATION_SCHEMA,
    evidenceResolution: "content_addressed",
    patchSha256,
    authorAgentId: isNonEmptyString(authorAgentId) ? authorAgentId : null,
    verdict: verdictFor(evaluatedLayers),
    thresholds: VERIFICATION_THRESHOLDS,
    layers: evaluatedLayers,
  };
  return deepFreeze({
    ...body,
    reportSha256: sha256(body),
  });
};

export const createVerificationEvidenceArtifact = (evidence) => {
  if (!evidence || typeof evidence !== "object" || Array.isArray(evidence)) {
    throw new TypeError("evidence must be an object");
  }
  const payload = canonicalJson(evidence);
  return Object.freeze({
    sha256: sha256(payload),
    payload,
  });
};

export const createVerificationReport = evaluateVerificationContract;

export const serializeVerificationReport = (report) => canonicalJson(report);

export const verifyVerificationReportHash = (report) => {
  if (!report || typeof report !== "object" || !isSha256(report.reportSha256)) return false;
  const { reportSha256, ...body } = report;
  return sha256(body) === reportSha256;
};
