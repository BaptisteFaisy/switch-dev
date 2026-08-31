import process from "node:process";
import path from "node:path";
import { pathToFileURL } from "node:url";

import { assertDevelopmentSwitchBaseUrl } from "./dev-safety.mjs";
import { createMassSubagentHttpServer } from "./http-server.mjs";
import { loadRoleRegistry } from "./roles.mjs";
import { MassSubagentScheduler } from "./scheduler.mjs";
import { MassSubagentService } from "./service.mjs";
import { JsonRunStore } from "./store.mjs";
import { createSwitchClient } from "./switch-client.mjs";

const STARTUP_TIMEOUT_MS = 20_000;

const integerEnv = (name, fallback, minimum, maximum) => {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") return fallback;
  const parsed = Number(raw);
  if (!Number.isInteger(parsed) || parsed < minimum || parsed > maximum) {
    throw new Error(`${name} must be between ${minimum} and ${maximum}`);
  }
  return parsed;
};

const booleanEnv = (name, fallback = false) => {
  const raw = process.env[name];
  if (raw === undefined || raw.trim() === "") return fallback;
  if (raw === "true") return true;
  if (raw === "false") return false;
  throw new Error(`${name} must be true or false`);
};

const assertNoToolScopeCapability = (health) => {
  if (!Array.isArray(health?.chatTurnToolScopes)
    || !health.chatTurnToolScopes.includes("none")) {
    throw new Error("Switch development does not advertise chat toolScope=none");
  }
};

const configuration = () => {
  if (process.env.SWITCH_ENV !== "development") {
    throw new Error("SWITCH_ENV=development is mandatory");
  }
  const bind = process.env.MASS_SUBAGENTS_BIND ?? "127.0.0.1";
  if (!["127.0.0.1", "0.0.0.0"].includes(bind)) {
    throw new Error("MASS_SUBAGENTS_BIND must be 127.0.0.1 or 0.0.0.0");
  }
  const switchBaseUrl = process.env.SWITCH_BASE_URL?.trim() || null;
  if (switchBaseUrl) assertDevelopmentSwitchBaseUrl(switchBaseUrl);
  const realDispatchEnabled = booleanEnv("MASS_SUBAGENTS_SWITCH_READONLY_ENABLED", false);
  const switchAdminToken = process.env.SWITCH_ADMIN_TOKEN || undefined;
  if (realDispatchEnabled && (!switchBaseUrl || !switchAdminToken)) {
    throw new Error(
      "SWITCH_BASE_URL and SWITCH_ADMIN_TOKEN are required when read-only dispatch is enabled",
    );
  }
  return {
    bind,
    port: integerEnv("MASS_SUBAGENTS_PORT", 18_084, 1, 65_535),
    dataDir: path.resolve(process.env.MASS_SUBAGENTS_DATA_DIR ?? "./data"),
    capacity: integerEnv("MASS_SUBAGENTS_SWITCH_CAPACITY", 2, 1, 1_024),
    microtasksPerSession: integerEnv("MASS_SUBAGENTS_MICROTASKS_PER_SESSION", 32, 1, 256),
    intervalMs: integerEnv("MASS_SUBAGENTS_TICK_MS", 1_000, 100, 60_000),
    maxRealAgents: integerEnv("MASS_SUBAGENTS_MAX_REAL_AGENTS", 32, 1, 10_000),
    maxStoredRuns: integerEnv("MASS_SUBAGENTS_MAX_STORED_RUNS", 100, 1, 100_000),
    maxArtifactBytes: integerEnv(
      "MASS_SUBAGENTS_MAX_ARTIFACT_BYTES",
      4 * 1024 * 1024,
      1_024,
      64 * 1024 * 1024,
    ),
    realDispatchEnabled,
    adminToken: process.env.MASS_SUBAGENTS_ADMIN_TOKEN ?? "",
    switchBaseUrl,
    switchAdminToken,
    releaseId: process.env.MASS_SUBAGENTS_RELEASE_ID ?? "development-unpackaged",
  };
};

const buildRuntime = async (config) => {
  const [store, registry] = await Promise.all([
    new JsonRunStore(config.dataDir, { maxArtifactBytes: config.maxArtifactBytes }).init(),
    loadRoleRegistry(),
  ]);
  const switchClient = config.switchBaseUrl
    ? createSwitchClient({
      baseUrl: config.switchBaseUrl,
      token: config.switchAdminToken,
      allowChatDispatch: config.realDispatchEnabled,
    })
    : null;
  let effectiveCapacity = config.capacity;
  if (config.realDispatchEnabled) {
    const health = await switchClient.health();
    const ready = health?.ready ?? health?.ok;
    if (ready !== true || health?.draining === true) {
      throw new Error("Switch development is not ready for read-only chat dispatch");
    }
    assertNoToolScopeCapability(health);
    if (Number.isInteger(health?.capacity) && health.capacity > 0) {
      effectiveCapacity = Math.min(effectiveCapacity, health.capacity);
    }
  }
  const scheduler = new MassSubagentScheduler({
    store,
    capacity: effectiveCapacity,
    microtasksPerSession: config.microtasksPerSession,
    intervalMs: config.intervalMs,
    switchClient,
  });
  const service = new MassSubagentService({
    store,
    scheduler,
    registry,
    switchClient,
    realDispatchEnabled: config.realDispatchEnabled,
    maxRealAgents: config.maxRealAgents,
    maxStoredRuns: config.maxStoredRuns,
  });
  return { store, registry, scheduler, service };
};

const serve = async () => {
  const startupWatchdog = setTimeout(() => {
    process.stderr.write("mass-subagents startup exceeded its bounded deadline\n");
    process.exit(1);
  }, STARTUP_TIMEOUT_MS);
  try {
    const config = configuration();
    const runtime = await buildRuntime(config);
    let shuttingDown = false;
    let server;
    const gracefulShutdown = async () => {
      if (shuttingDown) return;
      shuttingDown = true;
      const serverClosed = new Promise((resolve, reject) => {
        server.close((error) => {
          if (error) reject(error);
          else resolve();
        });
      });
      server.closeIdleConnections?.();
      await Promise.all([runtime.scheduler.stop(), serverClosed]);
      process.exit(0);
    };
    const requestShutdown = () => {
      gracefulShutdown().catch((error) => {
        process.stderr.write(`graceful shutdown failed: ${error.message}\n`);
        process.exitCode = 1;
      });
    };
    server = createMassSubagentHttpServer({
      service: runtime.service,
      scheduler: runtime.scheduler,
      adminToken: config.adminToken,
      realChatDispatchEnabled: config.realDispatchEnabled,
      onShutdownRequested: requestShutdown,
      releaseId: config.releaseId,
    });
    runtime.scheduler.start();
    await new Promise((resolve, reject) => {
      server.once("error", reject);
      server.listen(config.port, config.bind, resolve);
    });
    process.stdout.write(
      `Switch mass-subagents Phase 0/A listening on http://${config.bind}:${config.port}\n`,
    );
    process.once("SIGINT", requestShutdown);
    process.once("SIGTERM", requestShutdown);
  } finally {
    clearTimeout(startupWatchdog);
  }
};

const probe = async () => {
  const config = configuration();
  if (!config.switchBaseUrl) throw new Error("SWITCH_BASE_URL is required for probe-contract");
  const client = createSwitchClient({
    baseUrl: config.switchBaseUrl,
    token: config.switchAdminToken,
  });
  const result = await client.probeContract();
  process.stdout.write(`${JSON.stringify({
    ok: true,
    ready: result.health?.ready ?? result.health?.ok ?? null,
    capacity: result.health?.capacity ?? null,
    activeChatTurnCount: result.activeChatTurns.length,
    orchestrationCount: result.orchestrations.length,
  })}\n`);
};

if (process.argv[1] && pathToFileURL(path.resolve(process.argv[1])).href === import.meta.url) {
  const command = process.argv[2] ?? "serve";
  try {
    if (command === "serve") await serve();
    else if (command === "probe-contract") await probe();
    else throw new Error(`unknown command: ${command}`);
  } catch (error) {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  }
}

export const cliInternals = Object.freeze({
  assertNoToolScopeCapability,
  booleanEnv,
  buildRuntime,
  configuration,
});
