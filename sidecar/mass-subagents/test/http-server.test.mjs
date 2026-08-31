import assert from "node:assert/strict";
import { once } from "node:events";
import test from "node:test";

import { createMassSubagentHttpServer } from "../src/http-server.mjs";

const TOKEN = "phase-a-test-token-that-is-long-enough";

const withServer = async (callback, { onShutdownRequested = null } = {}) => {
  const service = {
    roles: () => ({ roles: [{ roleId: "scout" }] }),
    list: async () => ({ items: [], nextCursor: null, total: 0 }),
    create: async (body) => {
      if (body.idempotencyKey === "replay") {
        return { id: "run-http", status: "active", idempotentReplay: true };
      }
      if (body.executor === "real") {
        const error = new Error("real Switch chat dispatch is disabled");
        error.code = "REAL_DISPATCH_DISABLED";
        throw error;
      }
      return { id: "run-http", status: "active" };
    },
    get: async (id) => {
      if (id === "missing") {
        const error = new Error("missing");
        error.code = "ENOENT";
        throw error;
      }
      return { id, status: "active" };
    },
    agents: async () => ({ items: [], nextCursor: null, total: 0 }),
    control: async (id, action) => ({ id, status: action }),
    probeSwitchContract: async () => ({ capacity: 2 }),
  };
  const scheduler = { capacity: 2 };
  const server = createMassSubagentHttpServer({
    service,
    scheduler,
    adminToken: TOKEN,
    onShutdownRequested,
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  const origin = `http://127.0.0.1:${address.port}`;
  try {
    await callback(origin);
  } finally {
    server.close();
    await once(server, "close");
  }
};

const authenticated = (options = {}) => ({
  ...options,
  headers: {
    authorization: `Bearer ${TOKEN}`,
    ...options.headers,
  },
});

test("graceful shutdown is authenticated and acknowledged before the callback", async () => {
  let requested = false;
  await withServer(async (origin) => {
    assert.equal((await fetch(`${origin}/v1/admin/shutdown`, { method: "POST" })).status, 401);
    const response = await fetch(`${origin}/v1/admin/shutdown`, authenticated({ method: "POST" }));
    assert.equal(response.status, 202);
    assert.deepEqual(await response.json(), { ok: true, status: "shutting_down" });
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(requested, true);
  }, { onShutdownRequested: () => { requested = true; } });
});

test("the HTTP facade exposes only health without authentication", async () => {
  await withServer(async (origin) => {
    const health = await fetch(`${origin}/healthz`);
    assert.equal(health.status, 200);
    assert.deepEqual(await health.json(), {
      ok: true,
      environment: "development",
      executors: ["fake"],
      switchCapacity: 2,
      realChatDispatchEnabled: false,
      releaseId: "development-unpackaged",
    });
    assert.equal((await fetch(`${origin}/v1/roles`)).status, 401);
    const roles = await fetch(`${origin}/v1/roles`, authenticated());
    assert.equal(roles.status, 200);
    assert.equal((await roles.json()).roles[0].roleId, "scout");
  });
});

test("the HTTP facade maps validation, missing runs and disabled writes explicitly", async () => {
  await withServer(async (origin) => {
    const invalidCursor = await fetch(`${origin}/v1/runs?cursor=-1`, authenticated());
    assert.equal(invalidCursor.status, 400);
    assert.equal((await invalidCursor.json()).error.code, "INVALID_REQUEST");

    const missing = await fetch(`${origin}/v1/runs/missing`, authenticated());
    assert.equal(missing.status, 404);
    assert.equal((await missing.json()).error.code, "RUN_NOT_FOUND");

    const disabled = await fetch(`${origin}/v1/runs`, authenticated({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ executor: "real" }),
    }));
    assert.equal(disabled.status, 409);
    assert.equal((await disabled.json()).error.code, "REAL_DISPATCH_DISABLED");

    const replay = await fetch(`${origin}/v1/runs`, authenticated({
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ idempotencyKey: "replay" }),
    }));
    assert.equal(replay.status, 200);
    assert.equal((await replay.json()).idempotentReplay, true);
  });
});
