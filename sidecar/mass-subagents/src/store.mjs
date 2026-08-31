import {
  mkdir,
  link,
  open,
  readFile,
  readdir,
  rename,
  rm,
  stat,
  truncate,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { randomUUID } from "node:crypto";
import { setTimeout as delay } from "node:timers/promises";

import { canonicalJson, sha256 } from "./canonical.mjs";

const RUN_ID = /^[a-z0-9][a-z0-9_-]{0,127}$/i;
const LOCK_RETRY_MS = 10;
const LOCK_TIMEOUT_MS = 10_000;
const LOCK_SCHEMA = "switch-mass-subagents/run-lock/v1";
const TRANSACTION_SCHEMA = "switch-mass-subagents/store-transaction/v1";
const PROCESS_INSTANCE_ID = randomUUID();
const PROCESS_HOSTNAME = os.hostname();
const IMMUTABLE_RUN_FIELDS = Object.freeze([
  "id",
  "name",
  "projectDir",
  "createdAt",
  "definition",
  "configSha256",
  "instanceConfigSha256",
  "planSha256",
  "instancePlanSha256",
  "logicalAgentCount",
  "executor",
  "batches",
]);
const MUTABLE_AGENT_FIELDS = new Set([
  "status",
  "attemptCount",
  "availableAt",
  "outputSha256",
  "outputArtifactSha256",
  "waveArtifactSha256",
  "verificationStage",
  "failureReason",
  "dispatchToken",
  "dispatchStartedAt",
  "switchTurnId",
  "switchStatus",
  "lastPolledAt",
  "submittedAt",
  "attentionReason",
  "pollErrorCount",
]);

const requireRunId = (runId) => {
  if (typeof runId !== "string" || !RUN_ID.test(runId)) {
    throw new TypeError("invalid run id");
  }
  return runId;
};

const syncDirectory = async (directory) => {
  let handle;
  try {
    handle = await open(directory, "r");
    await handle.sync();
  } catch (error) {
    if (!["EINVAL", "EISDIR", "ENOTSUP", "EPERM"].includes(error?.code)) throw error;
  } finally {
    if (handle) await handle.close().catch(() => {});
  }
};

const syncFile = async (target) => {
  let handle;
  try {
    handle = await open(target, "r+");
    await handle.sync();
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  } finally {
    if (handle) await handle.close().catch(() => {});
  }
};

const atomicWrite = async (target, value) => {
  await mkdir(path.dirname(target), { recursive: true });
  const temporary = path.join(
    path.dirname(target),
    `.${path.basename(target)}.tmp-${process.pid}-${randomUUID()}`,
  );
  const payload = typeof value === "string" || Buffer.isBuffer(value)
    ? value
    : `${canonicalJson(value)}\n`;
  try {
    const handle = await open(temporary, "wx", 0o600);
    try {
      await handle.writeFile(payload);
      await handle.sync();
    } finally {
      await handle.close();
    }
    await rename(temporary, target);
    await syncDirectory(path.dirname(target));
  } catch (error) {
    await rm(temporary, { force: true }).catch(() => {});
    throw error;
  }
};

const serializeEvents = (events) => {
  if (!Array.isArray(events)) {
    const error = new TypeError("events must be an array");
    error.code = "STORE_EVENT_INVALID";
    throw error;
  }
  return events.map((event) => {
    if (event === null || typeof event !== "object" || Array.isArray(event)) {
      const error = new TypeError("every event must be a non-null object");
      error.code = "STORE_EVENT_INVALID";
      throw error;
    }
    const serialized = canonicalJson(event);
    if (typeof serialized !== "string" || serialized.length === 0) {
      const error = new TypeError("every event must have a canonical JSON representation");
      error.code = "STORE_EVENT_INVALID";
      throw error;
    }
    return serialized;
  });
};

const appendEvents = async (target, events) => {
  const serialized = serializeEvents(events);
  if (serialized.length === 0) return;
  const payload = `${serialized.join("\n")}\n`;
  const handle = await open(target, "a", 0o600);
  try {
    await handle.writeFile(payload);
    await handle.sync();
  } finally {
    await handle.close();
  }
};

const eventPayload = (events) => {
  const serialized = serializeEvents(events);
  return Buffer.from(serialized.length === 0 ? "" : `${serialized.join("\n")}\n`, "utf8");
};

const readFileOrEmpty = async (target) => {
  try {
    return await readFile(target);
  } catch (error) {
    if (error?.code === "ENOENT") return Buffer.alloc(0);
    throw error;
  }
};

const processStartMarkerForPid = async (pid) => {
  if (process.platform !== "linux" || !Number.isInteger(pid) || pid < 1) return null;
  try {
    const statLine = await readFile(`/proc/${pid}/stat`, "utf8");
    const commandEnd = statLine.lastIndexOf(")");
    if (commandEnd < 0) return null;
    const fieldsAfterCommand = statLine.slice(commandEnd + 2).trim().split(/\s+/u);
    return fieldsAfterCommand[19] ?? null;
  } catch {
    return null;
  }
};

const PROCESS_START_MARKER = await processStartMarkerForPid(process.pid);

const isValidLockOwner = (owner) => owner?.schema === LOCK_SCHEMA
  && typeof owner.token === "string"
  && /^[a-f0-9-]{36}$/u.test(owner.token)
  && typeof owner.processInstanceId === "string"
  && owner.processInstanceId.length > 0
  && Number.isInteger(owner.pid)
  && owner.pid > 0
  && typeof owner.hostname === "string"
  && owner.hostname.length > 0
  && typeof owner.candidateName === "string"
  && /^\.update\.lock\.candidate-[a-f0-9-]{36}$/u.test(owner.candidateName)
  && (owner.processStartMarker === null || typeof owner.processStartMarker === "string");

const clone = (value) => structuredClone(value);

const isProcessAlive = (pid) => {
  if (!Number.isInteger(pid) || pid < 1) return true;
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code !== "ESRCH";
  }
};

const immutableAgentProjection = (agent) => Object.fromEntries(
  Object.entries(agent).filter(([key]) => !MUTABLE_AGENT_FIELDS.has(key)),
);

const assertImmutableProjection = (current, next) => {
  for (const field of IMMUTABLE_RUN_FIELDS) {
    if (canonicalJson(current[field]) !== canonicalJson(next[field])) {
      throw new Error(`run field ${field} is immutable`);
    }
  }
  if (!Array.isArray(current.agents) || !Array.isArray(next.agents)
    || current.agents.length !== next.agents.length) {
    throw new Error("run agent identities are immutable");
  }
  for (let index = 0; index < current.agents.length; index += 1) {
    if (canonicalJson(immutableAgentProjection(current.agents[index]))
      !== canonicalJson(immutableAgentProjection(next.agents[index]))) {
      throw new Error(`run agent identity at index ${index} is immutable`);
    }
  }
};

const runListSummary = (record) => ({
  id: record.id,
  name: record.name,
  status: record.status,
  phase: record.phase,
  createdAt: record.createdAt,
  updatedAt: record.updatedAt,
  logicalAgentCount: record.logicalAgentCount,
  counters: record.counters,
  executor: record.executor,
  planSha256: record.planSha256,
  replaySha256: record.replaySha256 ?? null,
});

export class JsonRunStore {
  constructor(rootPath, {
    appendEventsImpl = appendEvents,
    writeProjectionImpl = atomicWrite,
    maxArtifactBytes = 4 * 1024 * 1024,
  } = {}) {
    if (typeof rootPath !== "string" || rootPath.trim().length === 0) {
      throw new TypeError("store root path is required");
    }
    this.rootPath = path.resolve(rootPath);
    this.runsPath = path.join(this.rootPath, "runs");
    this.artifactsPath = path.join(this.rootPath, "artifacts", "sha256");
    this.queues = new Map();
    this.appendEvents = appendEventsImpl;
    this.writeProjection = writeProjectionImpl;
    if (!Number.isInteger(maxArtifactBytes)
      || maxArtifactBytes < 1_024
      || maxArtifactBytes > 64 * 1024 * 1024) {
      throw new TypeError("maxArtifactBytes must be between 1024 and 67108864");
    }
    this.maxArtifactBytes = maxArtifactBytes;
  }

  async init() {
    await mkdir(this.runsPath, { recursive: true });
    await mkdir(this.artifactsPath, { recursive: true });
    return this;
  }

  async readRunUnlocked(runId) {
    const content = await readFile(path.join(this.runPath(runId), "run.json"), "utf8");
    const record = JSON.parse(content);
    const definition = JSON.parse(await readFile(
      path.join(this.runPath(runId), "definition.json"),
      "utf8",
    ));
    if (canonicalJson(record.definition ?? {}) !== canonicalJson(definition)) {
      const error = new Error(`immutable definition mismatch for ${runId}`);
      error.code = "STORE_DEFINITION_MISMATCH";
      throw error;
    }
    return record;
  }

  async writeSummaryBestEffort(record) {
    const target = path.join(this.runPath(record.id), "summary.json");
    try {
      await atomicWrite(target, runListSummary(record));
    } catch (error) {
      // summary.json is only a bounded listing cache. Removing it forces
      // list() to rebuild from the authoritative projection.
      await rm(target, { force: true }).catch(() => {});
    }
  }

  async recoverPendingTransactionUnlocked(runId, assertOwned = async () => {}) {
    const directory = this.runPath(runId);
    const pendingPath = path.join(directory, "transaction.pending.json");
    let transaction;
    try {
      transaction = JSON.parse(await readFile(pendingPath, "utf8"));
    } catch (error) {
      if (error?.code === "ENOENT") return null;
      throw new Error(`invalid pending transaction for ${runId}: ${error.message}`, {
        cause: error,
      });
    }
    if (transaction?.schema !== TRANSACTION_SCHEMA
      || transaction.runId !== runId
      || !Number.isSafeInteger(transaction.journalLengthBefore)
      || transaction.journalLengthBefore < 0
      || !Array.isArray(transaction.events)
      || sha256(transaction.previousProjection) !== transaction.previousProjectionSha256
      || sha256(transaction.nextProjection) !== transaction.nextProjectionSha256
      || transaction.nextProjection?.id !== runId
      || eventPayload(transaction.events).toString("hex") !== transaction.eventPayloadHex) {
      throw new Error(`pending transaction integrity check failed for ${runId}`);
    }

    const runPath = path.join(directory, "run.json");
    const eventsPath = path.join(directory, "events.ndjson");
    const current = await this.readRunUnlocked(runId);
    const currentSha256 = sha256(current);
    const expectedPayload = Buffer.from(transaction.eventPayloadHex, "hex");
    let journal = await readFileOrEmpty(eventsPath);
    if (journal.length < transaction.journalLengthBefore) {
      throw new Error(`event journal was truncated before pending transaction for ${runId}`);
    }
    const prefix = journal.subarray(0, transaction.journalLengthBefore);
    if (sha256(prefix) !== transaction.journalPrefixSha256) {
      throw new Error(`event journal prefix changed during pending transaction for ${runId}`);
    }
    let suffix = journal.subarray(transaction.journalLengthBefore);

    if (currentSha256 === transaction.nextProjectionSha256) {
      if (!suffix.equals(expectedPayload)) {
        throw new Error(`committed projection has inconsistent event journal for ${runId}`);
      }
      await assertOwned();
      await rm(pendingPath, { force: true });
      await syncDirectory(directory);
      await this.writeSummaryBestEffort(current);
      return current;
    }
    if (currentSha256 !== transaction.previousProjectionSha256) {
      throw new Error(`projection changed outside pending transaction for ${runId}`);
    }
    assertImmutableProjection(transaction.previousProjection, transaction.nextProjection);

    if (!suffix.equals(expectedPayload)) {
      // A process may have died during a single append. Only uncommitted bytes
      // after the durable prefix are removed; committed journal entries remain
      // append-only.
      await assertOwned();
      await truncate(eventsPath, transaction.journalLengthBefore).catch((error) => {
        if (error?.code !== "ENOENT") throw error;
      });
      await syncFile(eventsPath);
      await syncDirectory(directory);
      try {
        await assertOwned();
        await this.appendEvents(eventsPath, transaction.events);
      } catch (appendError) {
        journal = await readFileOrEmpty(eventsPath);
        suffix = journal.subarray(transaction.journalLengthBefore);
        if (!suffix.equals(expectedPayload)) throw appendError;
      }
      journal = await readFileOrEmpty(eventsPath);
      suffix = journal.subarray(transaction.journalLengthBefore);
      if (!suffix.equals(expectedPayload)) {
        throw new Error(`event journal append could not be recovered for ${runId}`);
      }
    }

    await assertOwned();
    await this.writeProjection(runPath, transaction.nextProjection);
    await this.writeSummaryBestEffort(transaction.nextProjection);
    await assertOwned();
    await rm(pendingPath, { force: true });
    await syncDirectory(directory);
    return transaction.nextProjection;
  }

  runPath(runId) {
    return path.join(this.runsPath, requireRunId(runId));
  }

  async acquireRunLock(runId) {
    const directory = this.runPath(runId);
    await stat(path.join(directory, "run.json"));
    const target = path.join(directory, ".update.lock");
    const deadline = Date.now() + LOCK_TIMEOUT_MS;
    while (true) {
      const token = randomUUID();
      const candidateName = `.update.lock.candidate-${token}`;
      const candidatePath = path.join(directory, candidateName);
      const owner = {
        schema: LOCK_SCHEMA,
        token,
        processInstanceId: PROCESS_INSTANCE_ID,
        pid: process.pid,
        hostname: PROCESS_HOSTNAME,
        processStartMarker: PROCESS_START_MARKER,
        candidateName,
        createdAt: new Date().toISOString(),
      };
      await atomicWrite(candidatePath, owner);
      try {
        await link(candidatePath, target);
        await syncDirectory(this.runPath(runId));
      } catch (error) {
        await rm(candidatePath, { force: true }).catch(() => {});
        if (!["EEXIST", "EPERM"].includes(error?.code)) throw error;
        let serializedOwner;
        try {
          serializedOwner = await readFile(target, "utf8");
        } catch (readError) {
          if (readError?.code === "ENOENT") {
            // The previous holder released (or a reclaimer removed) the mutex
            // between our failed link and this read: nobody owns it, so retry
            // the acquisition. A missing mutex is free, never "invalid" — only
            // a present-but-unparseable lock fails closed below.
            if (Date.now() >= deadline) {
              const timeoutError = new Error(`timed out acquiring run lock for ${runId}`);
              timeoutError.code = "STORE_LOCK_TIMEOUT";
              throw timeoutError;
            }
            await delay(LOCK_RETRY_MS);
            continue;
          }
          if (["EACCES", "EBUSY", "EPERM"].includes(readError?.code)
            && Date.now() < deadline) {
            // Windows can transiently refuse a read while another process is
            // publishing or releasing a hard-link mutex. This is contention,
            // not evidence of a corrupt lock record.
            await delay(LOCK_RETRY_MS);
            continue;
          }
          throw readError;
        }
        let currentOwner;
        try {
          currentOwner = JSON.parse(serializedOwner);
        } catch {
          currentOwner = null;
        }
        if (!isValidLockOwner(currentOwner)) {
          const recoveryError = new Error(
            `run lock for ${runId} is invalid; explicit local recovery is required`,
          );
          recoveryError.code = "STORE_LOCK_RECOVERY_REQUIRED";
          throw recoveryError;
        }
        if (currentOwner.hostname !== PROCESS_HOSTNAME) {
          const recoveryError = new Error(
            `run lock for ${runId} belongs to another host; automatic recovery is refused`,
          );
          recoveryError.code = "STORE_LOCK_RECOVERY_REQUIRED";
          throw recoveryError;
        }
        const actualProcessStartMarker = await processStartMarkerForPid(currentOwner.pid);
        const confirmedDeadLocalOwner = !isProcessAlive(currentOwner.pid);
        const reusedLocalProcess = (
          (currentOwner.pid === process.pid
            && currentOwner.processInstanceId !== PROCESS_INSTANCE_ID)
          || (currentOwner.processStartMarker !== null
            && actualProcessStartMarker !== null
            && currentOwner.processStartMarker !== actualProcessStartMarker)
        );
        if (confirmedDeadLocalOwner || reusedLocalProcess) {
          const claimPath = `${target}.reclaim-${currentOwner.token}`;
          let claimHandle;
          try {
            claimHandle = await open(claimPath, "wx", 0o600);
            await claimHandle.writeFile(`${canonicalJson({
              token: currentOwner.token,
              processInstanceId: PROCESS_INSTANCE_ID,
              pid: process.pid,
            })}\n`);
            await claimHandle.sync();
            const finalOwner = JSON.parse(await readFile(target, "utf8"));
            const finalMarker = await processStartMarkerForPid(finalOwner.pid);
            const finalCandidatePath = path.join(directory, finalOwner.candidateName);
            const [targetStat, candidateStat] = await Promise.all([
              stat(target).catch(() => null),
              stat(finalCandidatePath).catch(() => null),
            ]);
            const stillDead = finalOwner.hostname === PROCESS_HOSTNAME
              && (!isProcessAlive(finalOwner.pid)
                || (finalOwner.pid === process.pid
                  && finalOwner.processInstanceId !== PROCESS_INSTANCE_ID)
                || (finalOwner.processStartMarker !== null
                  && finalMarker !== null
                  && finalOwner.processStartMarker !== finalMarker));
            const samePublishedMutex = targetStat
              && candidateStat
              && targetStat.dev === candidateStat.dev
              && targetStat.ino === candidateStat.ino;
            const confirmedOwner = JSON.parse(await readFile(target, "utf8"));
            if (finalOwner.token === currentOwner.token
              && confirmedOwner.token === currentOwner.token
              && samePublishedMutex
              && stillDead) {
              await rm(target, { force: true });
              await rm(finalCandidatePath, { force: true });
              await syncDirectory(directory);
            }
          } catch (claimError) {
            if (claimError?.code !== "EEXIST") throw claimError;
          } finally {
            if (claimHandle) {
              await claimHandle.close().catch(() => {});
              await rm(claimPath, { force: true }).catch(() => {});
              await syncDirectory(directory);
            }
          }
          continue;
        }
        if (Date.now() >= deadline) {
          const timeoutError = new Error(`timed out acquiring run lock for ${runId}`);
          timeoutError.code = "STORE_LOCK_TIMEOUT";
          throw timeoutError;
        }
        await delay(LOCK_RETRY_MS);
        continue;
      }
      const handle = await open(candidatePath, "r+");
      let released = false;
      let fencedError = null;
      const fenced = () => {
        const error = new Error(`run lock mutex was fenced for ${runId}`);
        error.code = "STORE_LOCK_FENCED";
        return error;
      };
      const assertOwned = async () => {
        if (released || fencedError) throw fencedError ?? fenced();
        let currentOwner;
        try {
          currentOwner = JSON.parse(await readFile(target, "utf8"));
        } catch {
          currentOwner = null;
        }
        const [targetStat, candidateStat] = await Promise.all([
          stat(target).catch(() => null),
          stat(candidatePath).catch(() => null),
        ]);
        if (currentOwner?.token !== owner.token
          || !targetStat
          || !candidateStat
          || targetStat.dev !== candidateStat.dev
          || targetStat.ino !== candidateStat.ino) {
          fencedError = fenced();
          throw fencedError;
        }
        try {
          currentOwner = JSON.parse(await readFile(target, "utf8"));
        } catch {
          currentOwner = null;
        }
        if (currentOwner?.token !== owner.token) {
          fencedError = fenced();
          throw fencedError;
        }
      };
      const release = async () => {
        if (released) return;
        released = true;
        await handle.close();
        let currentOwner;
        try {
          currentOwner = JSON.parse(await readFile(target, "utf8"));
        } catch {
          currentOwner = null;
        }
        const [targetStat, candidateStat] = await Promise.all([
          stat(target).catch(() => null),
          stat(candidatePath).catch(() => null),
        ]);
        if (currentOwner?.token === owner.token
          && targetStat
          && candidateStat
          && targetStat.dev === candidateStat.dev
          && targetStat.ino === candidateStat.ino) {
          await rm(target, { force: true });
          await syncDirectory(directory);
        }
        await rm(candidatePath, { force: true }).catch(() => {});
      };
      Object.defineProperty(release, "assertOwned", {
        value: assertOwned,
        enumerable: false,
      });
      return release;
    }
  }

  async create(record, events = []) {
    requireRunId(record?.id);
    const directory = this.runPath(record.id);
    const staging = path.join(
      this.runsPath,
      `.create-${record.id}-${process.pid}-${randomUUID()}`,
    );
    await mkdir(staging, { recursive: false, mode: 0o700 });
    try {
      const definitionHandle = await open(
        path.join(staging, "definition.json"),
        "wx",
        0o400,
      );
      try {
        await definitionHandle.writeFile(`${canonicalJson(record.definition ?? {})}\n`);
        await definitionHandle.sync();
      } finally {
        await definitionHandle.close();
      }
      await atomicWrite(path.join(staging, "run.json"), record);
      await atomicWrite(path.join(staging, "summary.json"), runListSummary(record));
      await this.appendEvents(path.join(staging, "events.ndjson"), events);
      await syncDirectory(staging);
      await rename(staging, directory);
      await syncDirectory(this.runsPath);
    } catch (error) {
      await rm(staging, { recursive: true, force: true });
      throw error;
    }
    return clone(record);
  }

  async get(runId) {
    requireRunId(runId);
    const release = await this.acquireRunLock(runId);
    try {
      await this.recoverPendingTransactionUnlocked(runId, release.assertOwned);
      await release.assertOwned();
      return await this.readRunUnlocked(runId);
    } finally {
      await release();
    }
  }

  async has(runId) {
    try {
      await stat(path.join(this.runPath(runId), "run.json"));
      return true;
    } catch (error) {
      if (error?.code === "ENOENT") return false;
      throw error;
    }
  }

  async list({ limit = 100, cursor = 0 } = {}) {
    if (!Number.isInteger(limit) || limit < 1 || limit > 500) {
      throw new TypeError("limit must be between 1 and 500");
    }
    if (!Number.isInteger(cursor) || cursor < 0) {
      throw new TypeError("cursor must be a non-negative integer");
    }
    const entries = await readdir(this.runsPath, { withFileTypes: true });
    const records = [];
    for (const entry of entries) {
      if (!entry.isDirectory() || !RUN_ID.test(entry.name)) continue;
      try {
        // summary.json is a disposable cache and can be stale after a crash.
        // Listing always derives its answer from the authoritative projection,
        // under the same recovery lock used by get().
        const record = await this.get(entry.name);
        records.push(runListSummary(record));
      } catch (error) {
        if (error?.code !== "ENOENT") throw error;
      }
    }
    records.sort((left, right) =>
      String(right.createdAt).localeCompare(String(left.createdAt))
      || left.id.localeCompare(right.id));
    const items = records.slice(cursor, cursor + limit);
    return {
      items,
      nextCursor: cursor + items.length < records.length ? cursor + items.length : null,
      total: records.length,
    };
  }

  async update(runId, mutate, events = []) {
    requireRunId(runId);
    if (typeof mutate !== "function") throw new TypeError("mutate must be a function");
    serializeEvents(events);
    const previousQueue = this.queues.get(runId) ?? Promise.resolve();
    const operation = previousQueue.then(async () => {
      const release = await this.acquireRunLock(runId);
      try {
        await this.recoverPendingTransactionUnlocked(runId, release.assertOwned);
        const current = await this.readRunUnlocked(runId);
        const draft = clone(current);
        const replacement = await mutate(draft);
        const next = replacement ?? draft;
        await release.assertOwned();
        if (next.id !== runId) throw new Error("run id is immutable");
        assertImmutableProjection(current, next);
        const projectionChanged = canonicalJson(current) !== canonicalJson(next);
        if (!projectionChanged && !(events?.length)) return clone(current);
        if (projectionChanged && !(events?.length)) {
          const error = new Error("a projection change requires at least one append-only event");
          error.code = "STORE_EVENT_REQUIRED";
          throw error;
        }
        const directory = this.runPath(runId);
        const runPath = path.join(directory, "run.json");

        if (events?.length) {
          const pendingPath = path.join(directory, "transaction.pending.json");
          const eventsPath = path.join(directory, "events.ndjson");
          const journalPrefix = await readFileOrEmpty(eventsPath);
          const payload = eventPayload(events);
          const transaction = {
            schema: TRANSACTION_SCHEMA,
            runId,
            previousProjection: current,
            previousProjectionSha256: sha256(current),
            nextProjection: next,
            nextProjectionSha256: sha256(next),
            journalLengthBefore: journalPrefix.length,
            journalPrefixSha256: sha256(journalPrefix),
            events,
            eventPayloadHex: payload.toString("hex"),
          };
          await release.assertOwned();
          await atomicWrite(pendingPath, transaction);
          try {
            await release.assertOwned();
            await this.appendEvents(eventsPath, events);
            await release.assertOwned();
          } catch (error) {
            const journal = await readFileOrEmpty(eventsPath);
            const suffix = journal.subarray(journalPrefix.length);
            if (suffix.equals(payload)) {
              await release.assertOwned();
              await this.writeProjection(runPath, next);
              await release.assertOwned();
              await rm(pendingPath, { force: true });
              await syncDirectory(directory);
            } else {
              await release.assertOwned();
              await truncate(eventsPath, journalPrefix.length).catch((truncateError) => {
                if (truncateError?.code !== "ENOENT") throw truncateError;
              });
              await syncFile(eventsPath);
              await syncDirectory(directory);
              await rm(pendingPath, { force: true });
              await syncDirectory(directory);
              throw error;
            }
          }
          if (await stat(pendingPath).then(() => true, () => false)) {
            await release.assertOwned();
            await this.writeProjection(runPath, next);
            await release.assertOwned();
            await rm(pendingPath, { force: true });
            await syncDirectory(directory);
          }
        } else {
          await release.assertOwned();
          await this.writeProjection(runPath, next);
        }

        await release.assertOwned();
        await this.writeSummaryBestEffort(next);
        return clone(next);
      } finally {
        await release();
      }
    });
    const queued = operation.catch(() => {});
    this.queues.set(runId, queued);
    try {
      return await operation;
    } finally {
      if (this.queues.get(runId) === queued) this.queues.delete(runId);
    }
  }

  async pageAgents(runId, { limit = 100, cursor = 0 } = {}) {
    if (!Number.isInteger(limit) || limit < 1 || limit > 500) {
      throw new TypeError("limit must be between 1 and 500");
    }
    if (!Number.isInteger(cursor) || cursor < 0) {
      throw new TypeError("cursor must be a non-negative integer");
    }
    const run = await this.get(runId);
    const agents = Array.isArray(run.agents) ? run.agents : [];
    const items = agents.slice(cursor, cursor + limit);
    return {
      items,
      nextCursor: cursor + items.length < agents.length ? cursor + items.length : null,
      total: agents.length,
    };
  }

  async putArtifact(payload, expectedSha256 = null) {
    const bytes = Buffer.isBuffer(payload) ? payload : Buffer.from(canonicalJson(payload));
    if (bytes.length > this.maxArtifactBytes) {
      const error = new Error("artifact exceeds the configured byte limit");
      error.code = "STORE_ARTIFACT_TOO_LARGE";
      throw error;
    }
    const digest = sha256(bytes);
    if (expectedSha256 && expectedSha256 !== digest) {
      throw new Error("artifact hash mismatch");
    }
    const directory = path.join(this.artifactsPath, digest.slice(0, 2));
    const target = path.join(directory, digest);
    await mkdir(directory, { recursive: true });
    const temporary = path.join(directory, `.${digest}.tmp-${process.pid}-${randomUUID()}`);
    let published = false;
    try {
      const handle = await open(temporary, "wx", 0o400);
      try {
        await handle.writeFile(bytes);
        await handle.sync();
      } finally {
        await handle.close();
      }
      for (let attempt = 0; attempt < 4; attempt += 1) {
        let existing;
        try {
          existing = await readFile(target);
        } catch (error) {
          if (error?.code !== "ENOENT") throw error;
        }
        if (existing && sha256(existing) === digest) {
          published = true;
          break;
        }
        if (existing) {
          const quarantine = `${target}.corrupt-${new Date().toISOString().replaceAll(/[:.]/gu, "-")}-${randomUUID()}`;
          try {
            await rename(target, quarantine);
          } catch (error) {
            if (!["ENOENT", "EEXIST", "EPERM"].includes(error?.code)) throw error;
            continue;
          }
        }
        try {
          await rename(temporary, target);
          await syncDirectory(directory);
          published = true;
          break;
        } catch (error) {
          if (!["EEXIST", "EPERM"].includes(error?.code)) throw error;
        }
      }
      if (!published) throw new Error("artifact could not be published atomically");
      const stored = await readFile(target);
      if (sha256(stored) !== digest) throw new Error("stored artifact is corrupted");
    } finally {
      await rm(temporary, { force: true }).catch(() => {});
    }
    return Object.freeze({ sha256: digest, path: target, size: bytes.length });
  }

  async readEvents(runId) {
    requireRunId(runId);
    if (!(await this.has(runId))) return [];
    const release = await this.acquireRunLock(runId);
    try {
      await this.recoverPendingTransactionUnlocked(runId, release.assertOwned);
      await release.assertOwned();
      const content = await readFile(path.join(this.runPath(runId), "events.ndjson"), "utf8");
      return content
        .split(/\r?\n/u)
        .filter(Boolean)
        .map((line) => JSON.parse(line));
    } catch (error) {
      if (error?.code === "ENOENT") return [];
      throw error;
    } finally {
      await release();
    }
  }
}

export const storeInternals = Object.freeze({
  atomicWrite,
  appendEvents,
  eventPayload,
  requireRunId,
  runListSummary,
});
