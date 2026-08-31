import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import process from "node:process";

import { canonicalJson, sha256 } from "../src/canonical.mjs";
import { assertDevelopmentSwitchBaseUrl } from "../src/dev-safety.mjs";
import { AgentFactory, RUN_SPEC_SCHEMA_VERSION } from "../src/factory.mjs";
import { DeterministicFakeProvider } from "../src/fake-provider.mjs";
import { loadRoleRegistry } from "../src/roles.mjs";
import { MassSubagentScheduler } from "../src/scheduler.mjs";
import { JsonRunStore } from "../src/store.mjs";
import { createSwitchClient } from "../src/switch-client.mjs";

const sidecarRoot = path.resolve(import.meta.dirname, "..");
const repositoryRoot = path.resolve(sidecarRoot, "..");
const DEFAULT_ARTIFACTS_ROOT = path.join(repositoryRoot, "artifacts", "verification");
const TEST_STATUSES = new Set(["passed", "failed", "blocked"]);
const SHA256 = /^[a-f0-9]{64}$/u;
const FOUNDATION_TEST_IDS = new Set([
  "FOUNDATION-SOURCE-INVENTORY-001",
  "FOUNDATION-SOURCE-MANIFEST-001",
  "FOUNDATION-SYNTAX-UNIT-001",
  "FOUNDATION-FAKE-001",
  "P0-REDACTION-001",
  "DEV-TARGET-ALLOWLIST-001",
  "A-FACTORY-001",
  "A-SCHEDULER-001",
]);

class BlockedError extends Error {
  constructor(message) {
    super(message);
    this.name = "BlockedError";
  }
}

const parseArguments = (argumentsList) => {
  const values = {
    phase: "0",
    seed: 42,
    scale: 10,
    artifactsRoot: DEFAULT_ARTIFACTS_ROOT,
    foundationOnly: false,
  };
  for (let index = 0; index < argumentsList.length; index += 1) {
    const argument = argumentsList[index];
    if (argument === "--foundation-only") {
      values.foundationOnly = true;
      continue;
    }
    const value = argumentsList[index + 1];
    if (value === undefined) throw new TypeError(`missing value for ${argument}`);
    index += 1;
    if (argument === "--phase") values.phase = value.toUpperCase();
    else if (argument === "--seed") values.seed = Number(value);
    else if (argument === "--scale") values.scale = Number(value);
    else if (argument === "--artifacts-root") values.artifactsRoot = path.resolve(value);
    else throw new TypeError(`unknown argument ${argument}`);
  }
  if (!["0", "A"].includes(values.phase)) throw new TypeError("phase must be 0 or A");
  if (!Number.isSafeInteger(values.seed) || values.seed < 0) {
    throw new TypeError("seed must be a non-negative safe integer");
  }
  if (!Number.isInteger(values.scale) || values.scale < 1 || values.scale > 10) {
    throw new TypeError("Phase A scale must be between 1 and 10");
  }
  const root = path.parse(values.artifactsRoot).root;
  if (values.artifactsRoot === root || values.artifactsRoot === repositoryRoot) {
    throw new TypeError("artifacts root must be a dedicated child directory");
  }
  return values;
};

const escapeXml = (value) => String(value)
  .replaceAll("&", "&amp;")
  .replaceAll("<", "&lt;")
  .replaceAll(">", "&gt;")
  .replaceAll('"', "&quot;")
  .replaceAll("'", "&apos;");

const redact = (candidate, secrets) => {
  let value = String(candidate ?? "");
  for (const secret of secrets) {
    if (secret) value = value.replaceAll(secret, "<redacted>");
  }
  return value
    .replace(/Bearer\s+[A-Za-z0-9._~+\/-]+/giu, "Bearer <redacted>")
    .replace(/([?&](?:token|key|secret)=)[^&\s]+/giu, "$1<redacted>");
};

const atomicWrite = async (target, contents) => {
  await mkdir(path.dirname(target), { recursive: true });
  const temporary = `${target}.tmp-${process.pid}-${Date.now()}`;
  try {
    await writeFile(temporary, contents, { flag: "wx", mode: 0o600 });
    await rename(temporary, target);
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
};

const writeJson = (target, value) => atomicWrite(target, `${canonicalJson(value)}\n`);

const walk = async (directory) => {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    if (["artifacts", "chat-factory", "data", "node_modules"].includes(entry.name)) continue;
    const target = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await walk(target));
    else if (entry.isFile()) files.push(target);
  }
  return files;
};

const sourceManifest = async () => {
  const files = (await walk(sidecarRoot)).sort((left, right) => left.localeCompare(right));
  return Promise.all(files.map(async (file) => ({
    path: path.relative(sidecarRoot, file).replaceAll(path.sep, "/"),
    sha256: createHash("sha256").update(await readFile(file)).digest("hex"),
  })));
};

const runCommand = async (command, args, { secrets }) => new Promise((resolve, reject) => {
  const startedAt = performance.now();
  const child = spawn(command, args, {
    cwd: sidecarRoot,
    env: { ...process.env, SWITCH_ENV: "development" },
    shell: false,
    windowsHide: true,
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk; });
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  child.once("error", reject);
  child.once("close", (code, signal) => resolve({
    code,
    signal,
    durationMs: performance.now() - startedAt,
    stdout: redact(stdout, secrets),
    stderr: redact(stderr, secrets),
  }));
});

const executeTest = async (id, name, callback, { required = true } = {}) => {
  const startedAt = performance.now();
  try {
    const details = await callback();
    return {
      id,
      name,
      required,
      status: "passed",
      durationMs: Math.round((performance.now() - startedAt) * 1000) / 1000,
      details: details ?? {},
    };
  } catch (error) {
    return {
      id,
      name,
      required,
      status: error instanceof BlockedError ? "blocked" : "failed",
      durationMs: Math.round((performance.now() - startedAt) * 1000) / 1000,
      message: error.message,
    };
  }
};

const phaseASpecification = ({ runId, seed, scale }) => ({
  schemaVersion: RUN_SPEC_SCHEMA_VERSION,
  runId,
  seed,
  baseCommit: "a".repeat(40),
  context: {
    objective: "Verify the bounded mass-subagent factory with the deterministic provider",
    acceptanceCriteria: ["Every logical agent reaches one explicit terminal state"],
    constraints: ["Switch development only", "No real provider or generated code execution"],
  },
  batches: [{
    id: "phase-a-scouts",
    dependsOn: [],
    assignments: [{
      id: "bounded-readers",
      roleId: "scout",
      roleVersion: "1.0",
      count: scale,
      scope: ["sidecar/src/**"],
      context: {
        task: "Inspect one deterministic logical cell",
        acceptanceCriteria: ["Return the configured read-only contract"],
      },
    }],
  }],
});

const runtimeRecord = (plan, scenario) => ({
  id: plan.runId,
  name: "Phase A verification",
  createdAt: "2026-08-31T00:00:00.000Z",
  updatedAt: "2026-08-31T00:00:00.000Z",
  status: "active",
  phase: "phase_a_queued",
  logicalAgentCount: plan.logicalAgentCount,
  configSha256: sha256({ planSha256: plan.planSha256, scenario }),
  planSha256: plan.planSha256,
  definition: { seed: plan.seed, fakeScenario: scenario },
  executor: { kind: "fake", capacity: 2, microtasksPerSession: 2 },
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

const executePhaseARun = async ({ registry, seed, scale, rootPath }) => {
  const specification = phaseASpecification({
    runId: "run-phase-a-verification",
    seed,
    scale,
  });
  const plan = new AgentFactory({ registry }).expand(specification);
  const scenario = {
    throttleEvery: 3,
    timeoutEvery: 5,
    duplicateEvery: 4,
    outOfOrderEvery: 2,
    latencyEvery: 2,
    latencyMs: 25,
  };
  const store = await new JsonRunStore(rootPath).init();
  await store.create(runtimeRecord(plan, scenario), [{
    sequence: 1,
    timestamp: "2026-08-31T00:00:00.000Z",
    kind: "run_created",
  }]);
  let now = Date.parse("2026-08-31T00:00:00.000Z");
  const scheduler = new MassSubagentScheduler({
    store,
    capacity: 2,
    microtasksPerSession: 2,
    clock: () => now,
  });
  for (let iteration = 0; iteration < 100; iteration += 1) {
    await scheduler.tickRun(plan.runId);
    const run = await store.get(plan.runId);
    if (["completed", "needs_attention"].includes(run.status)) return run;
    now += 1_000;
  }
  throw new Error("Phase A scheduler did not converge within 100 deterministic ticks");
};

const convergeFakeProvider = ({ seed, scenario, tasks }) => {
  const provider = new DeterministicFakeProvider({ seed, scenario });
  const agents = [];
  const features = {
    throttles: 0,
    timeouts: 0,
    duplicates: 0,
    outOfOrder: 0,
    latency: 0,
  };
  for (const task of tasks) {
    const attempts = [];
    let terminal;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      const outcome = provider.execute({ ...task, attempt });
      attempts.push({ status: outcome.status, reason: outcome.reason ?? null });
      if (outcome.reason === "rate_limited") features.throttles += 1;
      if (outcome.reason === "timeout") features.timeouts += 1;
      if (outcome.status === "success") {
        if (outcome.duplicateDelivery) features.duplicates += 1;
        if (outcome.outOfOrderDelivery) features.outOfOrder += 1;
        if (outcome.latencyMs > 0) features.latency += 1;
        terminal = {
          logicalKey: task.logicalKey,
          attempts,
          payloadSha256: outcome.payloadSha256,
          duplicateDelivery: outcome.duplicateDelivery,
          outOfOrderDelivery: outcome.outOfOrderDelivery,
          latencyMs: outcome.latencyMs,
        };
        break;
      }
    }
    if (!terminal) throw new Error(`fake provider did not converge for ${task.logicalKey}`);
    agents.push(terminal);
  }
  return { agents, features, replaySha256: sha256({ agents, features }) };
};

const junit = (tests, suiteName) => {
  const failures = tests.filter(({ status }) => status === "failed").length;
  const skipped = tests.filter(({ status }) => status === "blocked").length;
  const duration = tests.reduce((sum, test) => sum + test.durationMs, 0) / 1000;
  const cases = tests.map((test) => {
    const body = test.status === "failed"
      ? `<failure message="${escapeXml(test.message)}"/>`
      : test.status === "blocked"
        ? `<skipped message="${escapeXml(test.message)}"/>`
        : "";
    return `  <testcase classname="${escapeXml(suiteName)}" name="${escapeXml(test.id)}" time="${(test.durationMs / 1000).toFixed(6)}">${body}</testcase>`;
  }).join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>\n<testsuite name="${escapeXml(suiteName)}" tests="${tests.length}" failures="${failures}" skipped="${skipped}" time="${duration.toFixed(6)}">\n${cases}\n</testsuite>\n`;
};

const verify = async (options) => {
  if (process.env.SWITCH_ENV !== "development") {
    throw new Error("SWITCH_ENV=development is mandatory");
  }
  if ((process.env.MASS_SUBAGENTS_EXECUTOR ?? "fake") !== "fake") {
    throw new Error("only the fake executor is permitted by the Phase 0/A verifier");
  }

  const secrets = [
    process.env.SWITCH_ADMIN_TOKEN,
    process.env.MASS_SUBAGENTS_ADMIN_TOKEN,
    "P0-SECRET-001-decoy-do-not-persist",
  ].filter(Boolean);
  const runLabel = `${new Date().toISOString().replaceAll(/[:.]/gu, "-")}-${process.pid}`;
  const phaseDirectory = path.join(options.artifactsRoot, `phase-${options.phase}`, runLabel);
  await mkdir(path.dirname(phaseDirectory), { recursive: true, mode: 0o700 });
  await mkdir(phaseDirectory, { recursive: false, mode: 0o700 });
  const tests = [];
  const logs = [];

  tests.push(await executeTest("FOUNDATION-SOURCE-INVENTORY-001", "sidecar source inventory", async () => {
    const required = [
      "package.json",
      "src/factory.mjs",
      "src/scheduler.mjs",
      "src/store.mjs",
      "test/roles-factory.test.mjs",
      "contracts/switch-http.v1.json",
    ];
    for (const relative of required) await stat(path.join(sidecarRoot, relative));
    return { files: required.length };
  }));

  tests.push(await executeTest("FOUNDATION-SOURCE-MANIFEST-001", "reproducible source manifest", async () => {
    const first = await sourceManifest();
    const second = await sourceManifest();
    const firstSha256 = sha256(first);
    const secondSha256 = sha256(second);
    if (firstSha256 !== secondSha256) throw new Error("source manifest changed during verification");
    return { files: first.length, sourceManifestSha256: firstSha256 };
  }));

  tests.push(await executeTest("FOUNDATION-SYNTAX-UNIT-001", "syntax and unit tests", async () => {
    const testFiles = (await readdir(path.join(sidecarRoot, "test")))
      .filter((name) => name.endsWith(".test.mjs"))
      .sort()
      .map((name) => path.join("test", name));
    const check = await runCommand(process.execPath, ["scripts/check-sources.mjs"], { secrets });
    logs.push(check.stdout, check.stderr);
    if (check.code !== 0) throw new Error(`source check failed with exit code ${check.code}`);
    const unit = await runCommand(process.execPath, ["--test", ...testFiles], { secrets });
    logs.push(unit.stdout, unit.stderr);
    if (unit.code !== 0) throw new Error(`unit tests failed with exit code ${unit.code}`);
    return { testFiles: testFiles.length, durationMs: check.durationMs + unit.durationMs };
  }));

  tests.push(await executeTest("FOUNDATION-FAKE-001", "deterministic fake-provider convergence", async () => {
    const tasks = Array.from({ length: 100 }, (_, index) => ({
      logicalKey: `logical-${index + 1}`,
      ordinal: index + 1,
      idempotencyKey: `fake-${index + 1}`,
    }));
    const scenario = {
      throttleEvery: 7,
      timeoutEvery: 11,
      duplicateEvery: 5,
      outOfOrderEvery: 3,
      latencyEvery: 2,
      latencyMs: 25,
    };
    const first = convergeFakeProvider({ seed: options.seed, scenario, tasks });
    const second = convergeFakeProvider({ seed: options.seed, scenario, tasks });
    if (canonicalJson(first) !== canonicalJson(second)) {
      throw new Error("fake provider converged replay changed");
    }
    if (Object.values(first.features).some((count) => count < 1)) {
      throw new Error("fake provider did not exercise every configured failure/delivery mode");
    }
    return { tasks: tasks.length, ...first.features, replaySha256: first.replaySha256 };
  }));

  tests.push(await executeTest("P0-INT-READ-001", "authenticated live Switch read snapshot", async () => {
    const baseUrl = process.env.SWITCH_DEV_BASE_URL ?? process.env.SWITCH_BASE_URL;
    const token = process.env.SWITCH_ADMIN_TOKEN;
    if (!baseUrl || !token) {
      throw new BlockedError(
        "SWITCH_DEV_BASE_URL and SWITCH_ADMIN_TOKEN are required for the live contract gate",
      );
    }
    const safeOrigin = assertDevelopmentSwitchBaseUrl(baseUrl);
    const client = createSwitchClient({ baseUrl: safeOrigin, token });
    const result = await client.probeContract();
    const capacity = Number(result.health?.capacity);
    if (!Number.isInteger(capacity) || capacity < 1) {
      throw new Error("Switch health did not expose a positive integer capacity");
    }
    return {
      origin: safeOrigin,
      capacity,
      activeChatTurnCount: result.activeChatTurns.length,
      orchestrationCount: result.orchestrations.length,
    };
  }, { required: false }));

  tests.push(await executeTest("P0-REDACTION-001", "secret redaction primitive", async () => {
    const decoy = secrets.at(-1);
    const sample = redact(`Authorization: Bearer ${decoy}\n?token=${decoy}`, secrets);
    if (sample.includes(decoy) || !sample.includes("<redacted>")) {
      throw new Error("decoy secret was not redacted");
    }
    return { scannedSecretClasses: secrets.length };
  }));

  tests.push(await executeTest("DEV-TARGET-ALLOWLIST-001", "development target allowlist", async () => {
    const candidates = [
      process.env.SWITCH_DEV_BASE_URL,
      process.env.SWITCH_BASE_URL,
      "http://127.0.0.1:18082",
      "http://switch:8080",
      "https://pc-fixe-cst.tail3a8bdf.ts.net:10000",
    ].filter(Boolean);
    const origins = candidates.map(assertDevelopmentSwitchBaseUrl);
    return { allowedOrigins: [...new Set(origins)] };
  }));

  const gateOnlyBlockers = [
    [
      "P0-SOURCE-001",
      "clean source checkout and build",
      "the sidecar source is present, but no clean committed checkout/build proof exists",
    ],
    [
      "P0-QUALITY-001",
      "formatter, linter and static-analysis gate",
      "syntax and unit tests pass, but formatter, linter and static-analysis tools are not configured",
    ],
    [
      "P0-FAKE-001",
      "full fake-provider end-to-end evidence",
      "provider convergence is tested, but the independently replayed retained P0 fake evidence gate is incomplete",
    ],
    [
      "P0-INT-001",
      "exact Switch launch/pilot/workspace contract",
      "the authenticated launch, pilot, cancellation and workspace routes have not been captured from Switch development",
    ],
    [
      "P0-BUILD-001",
      "two clean-cache reproducible image builds",
      "two isolated no-cache image builds and their content hashes have not been recorded",
    ],
    [
      "P0-BASELINE-001",
      "versioned performance baseline",
      "baseline thresholds and repeated measurements have not been calibrated and approved",
    ],
    [
      "P0-EVIDENCE-001",
      "evidence retention and replay audit",
      "the complete evidence bundle has not been independently replayed and retention-verified",
    ],
    [
      "P0-REVIEWER-001",
      "independent reviewer calibration",
      "no independent reviewer calibration report is available for this exact snapshot",
    ],
    [
      "QR-CALIBRATION-001",
      "QR corpus calibration",
      "the versioned QR calibration corpus is absent",
    ],
    [
      "P0-SECRET-001",
      "artifact, patch and image secret scan",
      "redaction is tested, but full artifact, patch and image secret scans have not run",
    ],
    [
      "DEV-NO-PROD-001",
      "observed no-production-contact proof",
      "the allowlist is tested, but no independent network, SSH and Docker activity observation has been recorded",
    ],
  ];
  for (const [id, name, reason] of gateOnlyBlockers) {
    tests.push(await executeTest(id, name, async () => {
      throw new BlockedError(reason);
    }, { required: !options.foundationOnly }));
  }

  if (options.phase === "A") {
    const registry = await loadRoleRegistry();
    tests.push(await executeTest("A-FACTORY-001", "typed factory expansion", async () => {
      const specification = phaseASpecification({
        runId: "run-phase-a-verification",
        seed: options.seed,
        scale: options.scale,
      });
      const plan = new AgentFactory({ registry }).expand(specification);
      if (plan.logicalAgentCount !== options.scale) throw new Error("factory count mismatch");
      if (new Set(plan.agents.map(({ id }) => id)).size !== options.scale) {
        throw new Error("factory produced duplicate agent ids");
      }
      return {
        logicalAgentCount: plan.logicalAgentCount,
        planSha256: plan.planSha256,
        registrySha256: registry.registry_sha256,
      };
    }));

    tests.push(await executeTest("A-SCHEDULER-001", "bounded deterministic scheduler", async () => {
      const scratch = await mkdtemp(path.join(os.tmpdir(), "switch-mass-phase-a-"));
      try {
        const first = await executePhaseARun({
          registry,
          seed: options.seed,
          scale: options.scale,
          rootPath: path.join(scratch, "first"),
        });
        const second = await executePhaseARun({
          registry,
          seed: options.seed,
          scale: options.scale,
          rootPath: path.join(scratch, "second"),
        });
        if (first.status !== "completed" || second.status !== "completed") {
          throw new Error("scheduler did not complete every logical agent");
        }
        if (first.replaySha256 !== second.replaySha256) {
          throw new Error("scheduler deterministic replay hash changed");
        }
        if (first.counters.peakSessions > 2) {
          throw new Error("scheduler exceeded the configured Switch capacity");
        }
        return {
          logicalAgentCount: options.scale,
          replaySha256: first.replaySha256,
          peakSessions: first.counters.peakSessions,
          retries: first.counters.retries,
          duplicatesIgnored: first.counters.duplicatesIgnored,
        };
      } finally {
        await rm(scratch, { recursive: true, force: true });
      }
    }));
  }

  for (const test of tests) {
    if (!TEST_STATUSES.has(test.status)) throw new Error(`invalid test status ${test.status}`);
    if (test.message) test.message = redact(test.message, secrets);
  }
  const requiredFailures = tests.filter((test) => test.required && test.status !== "passed");
  const gateQualified = requiredFailures.length === 0 && !options.foundationOnly;
  const foundationTests = tests.filter((test) => FOUNDATION_TEST_IDS.has(test.id));
  const expectedFoundationTestCount = options.phase === "A" ? 8 : 6;
  const foundationPassed = foundationTests.length === expectedFoundationTestCount
    && foundationTests.every((test) => test.status === "passed");
  const status = gateQualified ? "passed" : foundationPassed ? "foundation_passed" : "failed";
  const metadata = {
    schemaVersion: "switch-mass-subagents/verification-metadata/v1",
    phase: options.phase,
    status,
    gateQualified,
    foundationPassed,
    foundationOnly: options.foundationOnly,
    seed: options.seed,
    scale: options.phase === "A" ? options.scale : null,
    environment: "development",
    provider: "fake",
    realChatDispatchEnabled: false,
    limitations: gateOnlyBlockers.map(([id, , reason]) => ({ id, reason })),
    nodeVersion: process.version,
    platform: `${process.platform}-${process.arch}`,
    createdAt: new Date().toISOString(),
    tests,
  };
  const metrics = {
    testCount: tests.length,
    passed: tests.filter((test) => test.status === "passed").length,
    failed: tests.filter((test) => test.status === "failed").length,
    blocked: tests.filter((test) => test.status === "blocked").length,
    durationMs: tests.reduce((sum, test) => sum + test.durationMs, 0),
    phaseAMetrics: tests.find(({ id }) => id === "A-SCHEDULER-001")?.details ?? null,
  };
  const logText = redact(logs.filter(Boolean).join("\n"), secrets);
  const files = {
    metadata: path.join(phaseDirectory, "metadata.json"),
    junit: path.join(phaseDirectory, "junit.xml"),
    metrics: path.join(phaseDirectory, "metrics.json"),
    logs: path.join(phaseDirectory, "logs.txt"),
  };
  await Promise.all([
    writeJson(files.metadata, metadata),
    atomicWrite(files.junit, junit(tests, `switch-mass-subagents-phase-${options.phase}`)),
    writeJson(files.metrics, metrics),
    atomicWrite(files.logs, logText),
  ]);
  const hashes = {};
  for (const [name, file] of Object.entries(files)) {
    hashes[name] = createHash("sha256").update(await readFile(file)).digest("hex");
    if (!SHA256.test(hashes[name])) throw new Error(`invalid ${name} artifact hash`);
  }
  await writeJson(path.join(phaseDirectory, "hashes.json"), hashes);
  return { phaseDirectory, status, gateQualified, foundationPassed, tests };
};

const options = parseArguments(process.argv.slice(2));
try {
  const result = await verify(options);
  process.stdout.write(`${JSON.stringify({
    status: result.status,
    gateQualified: result.gateQualified,
    foundationPassed: result.foundationPassed,
    artifacts: result.phaseDirectory,
  })}\n`);
  if (!result.foundationPassed || (!options.foundationOnly && !result.gateQualified)) {
    process.exitCode = 1;
  }
} catch (error) {
  process.stderr.write(`${redact(error.message, [
    process.env.SWITCH_ADMIN_TOKEN,
    process.env.MASS_SUBAGENTS_ADMIN_TOKEN,
  ].filter(Boolean))}\n`);
  process.exitCode = 1;
}

export const verifierInternals = Object.freeze({
  convergeFakeProvider,
  executePhaseARun,
  parseArguments,
  phaseASpecification,
  redact,
  sourceManifest,
  verify,
});
