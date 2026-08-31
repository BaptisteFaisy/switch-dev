import assert from "node:assert/strict";
import { randomUUID } from "node:crypto";
import {
  appendFile,
  chmod,
  link,
  mkdtemp,
  readFile,
  readdir,
  rm,
  utimes,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";

import { JsonRunStore, storeInternals } from "../src/store.mjs";

const withStore = async (callback) => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-"));
  try {
    const store = await new JsonRunStore(root).init();
    await callback(store, root);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
};

const publishCrashedLock = async (root, runId, owner) => {
  const directory = path.join(root, "runs", runId);
  const candidatePath = path.join(directory, owner.candidateName);
  const targetPath = path.join(directory, ".update.lock");
  await writeFile(candidatePath, `${JSON.stringify(owner)}\n`, { flag: "wx" });
  await link(candidatePath, targetPath);
  return targetPath;
};

test("store persists an immutable definition and append-only events", async () => {
  await withStore(async (store, root) => {
    const record = {
      id: "run-1",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: { seed: 42 },
      agents: [],
      status: "active",
    };
    await store.create(record, [{ sequence: 1, kind: "created" }]);
    await store.update("run-1", (draft) => {
      draft.status = "completed";
    }, [{ sequence: 2, kind: "completed" }]);

    assert.equal((await store.get("run-1")).status, "completed");
    assert.deepEqual(
      (await store.readEvents("run-1")).map((event) => event.kind),
      ["created", "completed"],
    );
    assert.deepEqual(
      JSON.parse(await readFile(path.join(root, "runs", "run-1", "definition.json"), "utf8")),
      { seed: 42 },
    );
    const listed = await store.list();
    assert.equal(listed.items.length, 1);
    assert.equal(listed.items[0].status, "completed");
    assert.equal(Object.hasOwn(listed.items[0], "agents"), false);
  });
});

test("missing reads do not reserve a run directory or block later creation", async () => {
  await withStore(async (store, root) => {
    await assert.rejects(() => store.get("future-run"), { code: "ENOENT" });
    assert.deepEqual(await store.readEvents("future-run"), []);
    assert.deepEqual(await readdir(path.join(root, "runs")), []);
    await store.create({
      id: "future-run",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    assert.equal((await store.get("future-run")).status, "active");
  });
});

test("non-object events cannot advance a projection", async () => {
  await withStore(async (store) => {
    await store.create({
      id: "run-invalid-event",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    await assert.rejects(
      () => store.update("run-invalid-event", (draft) => {
        draft.status = "completed";
      }, [undefined]),
      { code: "STORE_EVENT_INVALID" },
    );
    assert.equal((await store.get("run-invalid-event")).status, "active");
    assert.deepEqual(await store.readEvents("run-invalid-event"), []);
  });
});

test("agent pages are bounded and cursor based", async () => {
  await withStore(async (store) => {
    await store.create({
      id: "run-page",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: Array.from({ length: 1_000 }, (_, index) => ({ id: `agent-${index}` })),
    });
    const page = await store.pageAgents("run-page", { limit: 100, cursor: 900 });
    assert.equal(page.items.length, 100);
    assert.equal(page.nextCursor, null);
    await assert.rejects(() => store.pageAgents("run-page", { limit: 501 }), /limit/);
  });
});

test("a list fallback cannot overwrite a concurrently refreshed summary", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-summary-race-"));
  try {
    const listingStore = await new JsonRunStore(root).init();
    const updatingStore = await new JsonRunStore(root).init();
    await listingStore.create({
      id: "run-summary-race",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    await rm(path.join(root, "runs", "run-summary-race", "summary.json"));
    const originalGet = listingStore.get.bind(listingStore);
    listingStore.get = async (id) => {
      const stale = await originalGet(id);
      await updatingStore.update(id, (draft) => {
        draft.status = "completed";
      }, [{ sequence: 1, kind: "completed" }]);
      return stale;
    };
    assert.equal((await listingStore.list()).items[0].status, "active");
    listingStore.get = originalGet;
    assert.equal((await listingStore.list()).items[0].status, "completed");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("list ignores a stale summary after a committed projection", async () => {
  await withStore(async (store, root) => {
    await store.create({
      id: "run-stale-summary",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    const summaryPath = path.join(root, "runs", "run-stale-summary", "summary.json");
    const staleSummary = await readFile(summaryPath);
    await store.update("run-stale-summary", (draft) => {
      draft.status = "completed";
    }, [{ sequence: 1, kind: "completed" }]);
    await writeFile(summaryPath, staleSummary);
    assert.equal((await store.list()).items[0].status, "completed");
  });
});

test("artifacts are atomically content addressed and corrupt finals are quarantined", async () => {
  await withStore(async (store, root) => {
    const first = await store.putArtifact({ value: 42 });
    const second = await store.putArtifact({ value: 42 });
    assert.deepEqual(first, second);
    await assert.rejects(() => store.putArtifact("other", first.sha256), /hash mismatch/);
    await chmod(first.path, 0o600);
    await writeFile(first.path, "partial-corruption");
    const recovered = await store.putArtifact({ value: 42 });
    assert.equal(recovered.sha256, first.sha256);
    const siblings = await readdir(path.dirname(first.path));
    assert.ok(siblings.some((name) => name.startsWith(`${first.sha256}.corrupt-`)));
    assert.equal(
      (await readFile(recovered.path, "utf8")),
      JSON.stringify({ value: 42 }),
    );
    assert.ok(recovered.path.startsWith(root));
  });
});

test("artifact bytes are bounded before anything is published", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-artifact-limit-"));
  try {
    const store = await new JsonRunStore(root, { maxArtifactBytes: 1_024 }).init();
    await assert.rejects(
      () => store.putArtifact(Buffer.alloc(1_025)),
      { code: "STORE_ARTIFACT_TOO_LARGE" },
    );
    assert.deepEqual(await readdir(path.join(root, "artifacts", "sha256")), []);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("run definitions, plans and logical identities are immutable", async () => {
  await withStore(async (store) => {
    await store.create({
      id: "run-immutable",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: { seed: 42 },
      configSha256: "a".repeat(64),
      planSha256: "b".repeat(64),
      batches: [{ id: "batch", dependsOn: [] }],
      agents: [{ id: "agent", logicalKey: "logical", status: "queued", attemptCount: 0 }],
      status: "active",
    });
    for (const mutate of [
      (draft) => { draft.definition.seed = 7; },
      (draft) => { draft.planSha256 = "c".repeat(64); },
      (draft) => { draft.agents[0].logicalKey = "changed"; },
    ]) {
      await assert.rejects(() => store.update("run-immutable", mutate), /immutable/u);
    }
    const run = await store.get("run-immutable");
    assert.equal(run.definition.seed, 42);
    assert.equal(run.planSha256, "b".repeat(64));
    assert.equal(run.agents[0].logicalKey, "logical");
  });
});

test("a rejected event append cannot advance the run projection", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-atomic-"));
  try {
    const store = await new JsonRunStore(root, {
      appendEventsImpl: async (_target, events) => {
        if (events.some(({ kind }) => kind === "refuse")) throw new Error("append refused");
      },
    }).init();
    await store.create({
      id: "run-atomic",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    await assert.rejects(
      () => store.update("run-atomic", (draft) => {
        draft.status = "completed";
      }, [{ sequence: 1, kind: "refuse" }]),
      /append refused/u,
    );
    assert.equal((await store.get("run-atomic")).status, "active");
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a post-append error commits exactly once when the durable journal is complete", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-post-append-"));
  try {
    const store = await new JsonRunStore(root, {
      appendEventsImpl: async (target, events) => {
        await storeInternals.appendEvents(target, events);
        if (events.some(({ kind }) => kind === "completed")) {
          throw new Error("post-append failure");
        }
      },
    }).init();
    await store.create({
      id: "run-post-append",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    }, [{ sequence: 1, kind: "created" }]);
    const updated = await store.update("run-post-append", (draft) => {
      draft.status = "completed";
    }, [{ sequence: 2, kind: "completed" }]);
    assert.equal(updated.status, "completed");
    assert.deepEqual(
      (await store.readEvents("run-post-append")).map(({ kind }) => kind),
      ["created", "completed"],
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a partial append is rolled back without truncating committed events", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-partial-append-"));
  try {
    const store = await new JsonRunStore(root, {
      appendEventsImpl: async (target, events) => {
        if (events.some(({ kind }) => kind === "partial")) {
          const payload = storeInternals.eventPayload(events);
          await appendFile(target, payload.subarray(0, Math.floor(payload.length / 2)));
          throw new Error("partial append failure");
        }
        await storeInternals.appendEvents(target, events);
      },
    }).init();
    await store.create({
      id: "run-partial",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    }, [{ sequence: 1, kind: "created" }]);
    await assert.rejects(
      () => store.update("run-partial", (draft) => {
        draft.status = "completed";
      }, [{ sequence: 2, kind: "partial" }]),
      /partial append failure/u,
    );
    assert.equal((await store.get("run-partial")).status, "active");
    assert.deepEqual(await store.readEvents("run-partial"), [{ sequence: 1, kind: "created" }]);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a pending WAL transaction recovers after the journal append boundary", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-recovery-"));
  try {
    const initial = await new JsonRunStore(root).init();
    await initial.create({
      id: "run-recovery",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    }, [{ sequence: 1, kind: "created" }]);
    let failCommit = true;
    const interrupted = await new JsonRunStore(root, {
      writeProjectionImpl: async (target, value) => {
        if (failCommit && value.status === "completed") {
          failCommit = false;
          throw new Error("simulated crash before projection commit");
        }
        await storeInternals.atomicWrite(target, value);
      },
    }).init();
    await assert.rejects(
      () => interrupted.update("run-recovery", (draft) => {
        draft.status = "completed";
      }, [{ sequence: 2, kind: "completed" }]),
      /simulated crash/u,
    );

    const recovered = await new JsonRunStore(root).init();
    assert.equal((await recovered.get("run-recovery")).status, "completed");
    assert.deepEqual(
      (await recovered.readEvents("run-recovery")).map(({ kind }) => kind),
      ["created", "completed"],
    );
    await assert.rejects(
      () => readFile(path.join(root, "runs", "run-recovery", "transaction.pending.json")),
      (error) => error?.code === "ENOENT",
    );
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("two store instances serialize updates through the run lock", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-lock-"));
  try {
    const first = await new JsonRunStore(root).init();
    const second = await new JsonRunStore(root).init();
    await first.create({
      id: "run-lock",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      counter: 0,
    });
    await Promise.all([first, second].map((store) => store.update("run-lock", async (draft) => {
      const previous = draft.counter;
      await new Promise((resolve) => setTimeout(resolve, 20));
      draft.counter = previous + 1;
    }, [{ kind: "increment" }])));
    assert.equal((await first.get("run-lock")).counter, 2);
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a live lock is not stolen solely because its mtime looks stale", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-live-lock-"));
  try {
    const first = await new JsonRunStore(root).init();
    const second = await new JsonRunStore(root).init();
    await first.create({
      id: "run-live-lock",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
    });
    const releaseFirst = await first.acquireRunLock("run-live-lock");
    const lockPath = path.join(root, "runs", "run-live-lock", ".update.lock");
    const stale = new Date(Date.now() - 120_000);
    await utimes(lockPath, stale, stale);
    let secondAcquired = false;
    const pendingSecond = second.acquireRunLock("run-live-lock").then((release) => {
      secondAcquired = true;
      return release;
    });
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(secondAcquired, false);
    await releaseFirst();
    const releaseSecond = await pendingSecond;
    assert.equal(secondAcquired, true);
    await releaseSecond();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("invalid lock records fail closed and require explicit local recovery", async () => {
  await withStore(async (store, root) => {
    await store.create({
      id: "run-crashed-lock",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: { seed: 42 },
      agents: [],
      status: "active",
    });
    const lockPath = path.join(root, "runs", "run-crashed-lock", ".update.lock");
    for (const crashedPayload of ["", '{"schema":"switch-mass-subagents/run-lock/v1"']) {
      await writeFile(lockPath, crashedPayload, { flag: "wx" });
      await assert.rejects(
        () => store.acquireRunLock("run-crashed-lock"),
        { code: "STORE_LOCK_RECOVERY_REQUIRED" },
      );
      await rm(lockPath, { force: true });
    }
  });
});

test("a proven local PID reuse is recovered but a foreign owner fails closed", async () => {
  await withStore(async (store, root) => {
    await store.create({
      id: "run-restarted-owner",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    const lockPath = path.join(root, "runs", "run-restarted-owner", ".update.lock");
    const token = randomUUID();
    const baseOwner = {
      schema: "switch-mass-subagents/run-lock/v1",
      token,
      processInstanceId: "previous-process-instance",
      pid: process.pid,
      hostname: os.hostname(),
      processStartMarker: null,
      candidateName: `.update.lock.candidate-${token}`,
      createdAt: "2026-08-30T00:00:00.000Z",
    };
    await publishCrashedLock(root, "run-restarted-owner", baseOwner);
    const release = await store.acquireRunLock("run-restarted-owner");
    await release.assertOwned();
    await release();
    await assert.rejects(readFile(lockPath), { code: "ENOENT" });

    const foreignToken = randomUUID();
    const foreignOwner = {
      ...baseOwner,
      token: foreignToken,
      hostname: "previous-container-hostname",
      candidateName: `.update.lock.candidate-${foreignToken}`,
    };
    await publishCrashedLock(root, "run-restarted-owner", foreignOwner);
    await assert.rejects(
      () => store.acquireRunLock("run-restarted-owner"),
      { code: "STORE_LOCK_RECOVERY_REQUIRED" },
    );
    await rm(lockPath, { force: true });
  });
});

test("two reclaimers cannot remove the newly acquired hard-link mutex", async () => {
  const root = await mkdtemp(path.join(os.tmpdir(), "switch-mass-store-reclaim-race-"));
  try {
    const first = await new JsonRunStore(root).init();
    const second = await new JsonRunStore(root).init();
    await first.create({
      id: "run-reclaim-race",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
    });
    const token = randomUUID();
    const lockPath = path.join(root, "runs", "run-reclaim-race", ".update.lock");
    await publishCrashedLock(root, "run-reclaim-race", {
      schema: "switch-mass-subagents/run-lock/v1",
      token,
      processInstanceId: "previous-process-instance",
      pid: process.pid,
      hostname: os.hostname(),
      processStartMarker: null,
      candidateName: `.update.lock.candidate-${token}`,
      createdAt: "2026-08-30T00:00:00.000Z",
    });
    let secondResolved = false;
    const pending = [first, second].map((store, index) =>
      store.acquireRunLock("run-reclaim-race").then((release) => {
        if (index === 1) secondResolved = true;
        return { index, release };
      }));
    const winner = await Promise.race(pending);
    await winner.release.assertOwned();
    await new Promise((resolve) => setTimeout(resolve, 100));
    assert.equal(secondResolved, winner.index === 1);
    await winner.release();
    const loser = await pending[1 - winner.index];
    await loser.release.assertOwned();
    await loser.release();
  } finally {
    await rm(root, { recursive: true, force: true });
  }
});

test("a replaced hard-link mutex cannot publish a stale projection", async () => {
  await withStore(async (store, root) => {
    await store.create({
      id: "run-fenced-writer",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    const lockPath = path.join(root, "runs", "run-fenced-writer", ".update.lock");
    let candidatePath;
    await assert.rejects(
      store.update("run-fenced-writer", async (draft) => {
        const owner = JSON.parse(await readFile(lockPath, "utf8"));
        candidatePath = path.join(path.dirname(lockPath), owner.candidateName);
        await writeFile(lockPath, `${JSON.stringify({ ...owner, token: "replacement-token" })}\n`);
        draft.status = "must-not-persist";
      }, [{ kind: "must-not-persist" }]),
      { code: "STORE_LOCK_FENCED" },
    );
    const projection = JSON.parse(await readFile(
      path.join(root, "runs", "run-fenced-writer", "run.json"),
      "utf8",
    ));
    assert.equal(projection.status, "active");
    await rm(lockPath, { force: true });
    await rm(candidatePath, { force: true });
  });
});

test("projection changes without an append-only event are refused", async () => {
  await withStore(async (store) => {
    await store.create({
      id: "run-event-required",
      createdAt: "2026-08-31T00:00:00.000Z",
      definition: {},
      agents: [],
      status: "active",
    });
    await assert.rejects(
      () => store.update("run-event-required", (draft) => {
        draft.status = "completed";
      }),
      { code: "STORE_EVENT_REQUIRED" },
    );
    assert.equal((await store.get("run-event-required")).status, "active");
  });
});
