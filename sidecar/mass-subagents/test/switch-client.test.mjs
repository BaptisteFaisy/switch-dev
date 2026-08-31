import assert from "node:assert/strict";
import http from "node:http";
import { once } from "node:events";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  CANONICAL_SWITCH_DEVELOPMENT_ORIGIN,
  COMPOSE_SWITCH_DEVELOPMENT_ORIGIN,
  SwitchDevelopmentSafetyError,
  assertDevelopmentSwitchBaseUrl,
} from "../src/dev-safety.mjs";
import {
  SWITCH_HTTP_V1_ROUTES,
  SWITCH_HTTP_V1_OPERATIONS,
  SWITCH_HTTP_V2_OPERATIONS,
  SwitchClientError,
  createSwitchClient,
  validateStartChatTurnRequest,
} from "../src/switch-client.mjs";

const startFakeSwitch = async (handler) => {
  const requests = [];
  const server = http.createServer(async (request, response) => {
    const chunks = [];
    for await (const chunk of request) chunks.push(chunk);
    const body = Buffer.concat(chunks).toString("utf8");
    requests.push({
      method: request.method,
      path: request.url,
      authorization: request.headers.authorization,
      body,
    });
    await handler(request, response);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const { port } = server.address();
  return {
    baseUrl: `http://127.0.0.1:${port}`,
    requests,
    close: async () => {
      server.closeAllConnections?.();
      server.close();
      await once(server, "close");
    },
  };
};

const sendJson = (response, body, status = 200) => {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(body));
};

test("contract and client expose only the phase-0 read-only routes", async () => {
  const contract = JSON.parse(
    await readFile(new URL("../contracts/switch-http.v1.json", import.meta.url), "utf8"),
  );
  assert.equal(contract.operations.health.path, "/healthz");
  assert.deepEqual(
    Object.fromEntries(
      Object.entries(contract.operations).map(([name, operation]) => [name, operation.path]),
    ),
    SWITCH_HTTP_V1_ROUTES,
  );
  assert.deepEqual(contract.baseUrlPolicy.composeOrigins, [COMPOSE_SWITCH_DEVELOPMENT_ORIGIN]);
  assert.deepEqual(
    Object.fromEntries(Object.entries(contract.operations).map(([name, operation]) => [
      name,
      operation.authenticationRequired,
    ])),
    Object.fromEntries(Object.entries(SWITCH_HTTP_V1_OPERATIONS).map(([name, operation]) => [
      name,
      operation.authenticationRequired,
    ])),
  );
  assert.deepEqual(
    new Set(Object.values(contract.operations).map(({ method }) => method)),
    new Set(["GET"]),
  );
});

const readonlyRequest = (overrides = {}) => ({
  accountId: "account-dev",
  sessionId: null,
  prompt: "Return JSON only",
  imageAttachments: [],
  projectDir: "E:/switch-project",
  mode: "ask",
  toolScope: "none",
  model: "gpt-5.4",
  reasoningEffort: "medium",
  appConnectors: [],
  appWriteApproved: false,
  agentTools: [],
  agentSkills: [],
  questionTool: false,
  proofTool: false,
  sourceChatKey: "mass-subagents:one",
  ...overrides,
});

test("v2 declares the exact development chat lifecycle and keeps ask mode read-only", async () => {
  const contract = JSON.parse(
    await readFile(new URL("../contracts/switch-http.v2.json", import.meta.url), "utf8"),
  );
  assert.equal(contract.operations.startChatTurn.path, "/api/chat/turns");
  assert.equal(contract.operations.getChatTurn.pathTemplate, "/api/chat/turns/{id}");
  assert.equal(contract.operations.cancelChatTurn.pathTemplate, "/api/chat/turns/{id}");
  assert.equal(contract.safety.dispatchMode, "ask");
  assert.equal(contract.safety.toolScope, "none");
  assert.equal(contract.safety.automaticRetryAfterUnknownOutcome, false);
  for (const [name, operation] of Object.entries(SWITCH_HTTP_V2_OPERATIONS)) {
    assert.equal(contract.operations[name].method, operation.method);
    assert.equal(contract.operations[name].path, operation.path);
    assert.equal(contract.operations[name].pathTemplate, operation.pathTemplate);
    assert.equal(contract.operations[name].authenticationRequired, operation.authenticationRequired);
  }
});

test("chat writes fail closed unless explicitly enabled and validated", async (t) => {
  const fake = await startFakeSwitch((_request, response) => sendJson(response, {
    id: 1,
    sourceChatKey: "mass-subagents:one",
    status: "running",
  }));
  t.after(fake.close);
  const disabled = createSwitchClient({ baseUrl: fake.baseUrl, token: "dev-token" });
  await assert.rejects(disabled.startChatTurn(readonlyRequest()), (error) => {
    assert.equal(error.code, "SWITCH_WRITE_DISABLED");
    return true;
  });
  assert.equal(fake.requests.length, 0);
  assert.throws(
    () => validateStartChatTurnRequest(readonlyRequest({ mode: "build" })),
    /read-only ask mode/u,
  );
  assert.throws(
    () => validateStartChatTurnRequest(readonlyRequest({ appWriteApproved: true })),
    /must be false/u,
  );
  assert.throws(
    () => validateStartChatTurnRequest(readonlyRequest({ toolScope: "full" })),
    /toolScope=none/u,
  );
  assert.throws(
    () => validateStartChatTurnRequest(readonlyRequest({ toolScope: undefined })),
    /toolScope=none/u,
  );
});

test("enabled chat lifecycle sends the exact DTO and authenticated routes", async (t) => {
  const fake = await startFakeSwitch((request, response) => {
    if (request.method === "POST") {
      return sendJson(response, { id: 17, sourceChatKey: "mass-subagents:one", status: "running" });
    }
    if (request.method === "DELETE") {
      return sendJson(response, { id: 17, sourceChatKey: "mass-subagents:one", status: "cancelled" });
    }
    return sendJson(response, {
      id: 17,
      sourceChatKey: "mass-subagents:one",
      status: "completed",
      parts: [],
    });
  });
  t.after(fake.close);
  const client = createSwitchClient({
    baseUrl: fake.baseUrl,
    token: "dev-token",
    allowChatDispatch: true,
  });
  assert.equal((await client.startChatTurn(readonlyRequest())).id, 17);
  assert.equal((await client.getChatTurn(17)).status, "completed");
  assert.equal((await client.cancelChatTurn("17")).status, "cancelled");
  assert.deepEqual(fake.requests.map(({ method, path }) => [method, path]), [
    ["POST", "/api/chat/turns"],
    ["GET", "/api/chat/turns/17"],
    ["DELETE", "/api/chat/turns/17"],
  ]);
  assert.deepEqual(JSON.parse(fake.requests[0].body), readonlyRequest());
  assert.ok(fake.requests.every(({ authorization }) => authorization === "Bearer dev-token"));
});

test("probeContract requires auth and reads all read-only contract routes", async (t) => {
  const fake = await startFakeSwitch((request, response) => {
    if (request.url === "/healthz") return sendJson(response, { ok: true });
    if (request.url === "/api/chat/turns/active") {
      return sendJson(response, [{ id: "1", status: "running", sourceChatKey: null }]);
    }
    if (request.url === "/api/orchestrations") return sendJson(response, [{ id: "orch-1" }]);
    return sendJson(response, { error: "not found" }, 404);
  });
  t.after(fake.close);

  const client = createSwitchClient({ baseUrl: fake.baseUrl });
  await assert.rejects(client.probeContract(), (error) => {
    assert.equal(error.code, "SWITCH_AUTH_REQUIRED");
    return true;
  });
  assert.equal(fake.requests.length, 0);

  const authenticated = createSwitchClient({ baseUrl: fake.baseUrl, token: "dev-token" });
  assert.deepEqual(await authenticated.probeContract(), {
    health: { ok: true },
    activeChatTurns: [{ id: "1", status: "running", sourceChatKey: null }],
    orchestrations: [{ id: "orch-1" }],
  });
  assert.deepEqual(fake.requests.map(({ method }) => method), ["GET", "GET", "GET"]);
  assert.equal(fake.requests.find(({ path }) => path === "/healthz").authorization, undefined);
  assert.ok(fake.requests
    .filter(({ path }) => path.startsWith("/api/"))
    .every(({ authorization }) => authorization === "Bearer dev-token"));
});

test("API bearer auth is sent but never exposed by an HTTP error", async (t) => {
  const secret = "test-secret-that-must-not-leak";
  const fake = await startFakeSwitch((request, response) => {
    if (request.headers.authorization !== `Bearer ${secret}`) {
      return sendJson(response, { echoedToken: request.headers.authorization }, 401);
    }
    return sendJson(response, { denied: true }, 403);
  });
  t.after(fake.close);

  const client = createSwitchClient({ baseUrl: fake.baseUrl, token: secret });
  await assert.rejects(client.listActiveChatTurns(), (error) => {
    assert.ok(error instanceof SwitchClientError);
    assert.equal(error.code, "SWITCH_HTTP_ERROR");
    assert.equal(error.status, 403);
    assert.doesNotMatch(error.message, new RegExp(secret));
    assert.doesNotMatch(JSON.stringify(error), new RegExp(secret));
    return true;
  });
  assert.equal(fake.requests[0].authorization, `Bearer ${secret}`);
});

test("direct API reads reject missing auth before network while health stays public", async (t) => {
  const fake = await startFakeSwitch((_request, response) => sendJson(response, { ok: true }));
  t.after(fake.close);
  const client = createSwitchClient({ baseUrl: fake.baseUrl });
  await assert.rejects(client.listActiveChatTurns(), (error) => {
    assert.equal(error.code, "SWITCH_AUTH_REQUIRED");
    return true;
  });
  assert.equal(fake.requests.length, 0);
  assert.deepEqual(await client.health(), { ok: true });
  assert.equal(fake.requests.length, 1);
  assert.equal(fake.requests[0].authorization, undefined);
});

test("production and non-canonical network targets are refused before fetch", () => {
  let fetchCalls = 0;
  const fetchImpl = async () => {
    fetchCalls += 1;
    throw new Error("must not run");
  };

  for (const baseUrl of [
    "https://switch.example.com",
    "http://10.0.0.8:8080",
    "https://pc-fixe-cst.tail3a8bdf.ts.net",
    "http://pc-fixe-cst.tail3a8bdf.ts.net:10000",
    "http://127.0.0.2:8080",
    "http://2130706433:8080",
  ]) {
    assert.throws(
      () => createSwitchClient({ baseUrl, fetchImpl }),
      SwitchDevelopmentSafetyError,
    );
  }
  assert.equal(fetchCalls, 0);
  assert.equal(
    assertDevelopmentSwitchBaseUrl(`${CANONICAL_SWITCH_DEVELOPMENT_ORIGIN}/`),
    CANONICAL_SWITCH_DEVELOPMENT_ORIGIN,
  );
  assert.equal(
    assertDevelopmentSwitchBaseUrl(COMPOSE_SWITCH_DEVELOPMENT_ORIGIN),
    COMPOSE_SWITCH_DEVELOPMENT_ORIGIN,
  );
});

test("requests time out with a redacted, typed error", async (t) => {
  const fake = await startFakeSwitch((_request, response) => {
    setTimeout(() => sendJson(response, { tooLate: true }), 100);
  });
  t.after(fake.close);

  const client = createSwitchClient({ baseUrl: fake.baseUrl, timeoutMs: 20 });
  await assert.rejects(client.health(), (error) => {
    assert.ok(error instanceof SwitchClientError);
    assert.equal(error.code, "SWITCH_TIMEOUT");
    return true;
  });
});

test("oversized Switch responses are rejected before JSON decoding", async (t) => {
  const fake = await startFakeSwitch((_request, response) => {
    sendJson(response, { payload: "x".repeat(2_000) });
  });
  t.after(fake.close);
  const client = createSwitchClient({ baseUrl: fake.baseUrl, maxResponseBytes: 1_024 });
  await assert.rejects(client.health(), (error) => {
    assert.equal(error.code, "SWITCH_RESPONSE_TOO_LARGE");
    return true;
  });
});

test("a caller AbortSignal cancels an in-flight request", async (t) => {
  const fake = await startFakeSwitch((_request, response) => {
    setTimeout(() => sendJson(response, { tooLate: true }), 100);
  });
  t.after(fake.close);

  const controller = new AbortController();
  const client = createSwitchClient({ baseUrl: fake.baseUrl, timeoutMs: 500 });
  const pending = client.health({ signal: controller.signal });
  controller.abort();
  await assert.rejects(pending, (error) => {
    assert.ok(error instanceof SwitchClientError);
    assert.equal(error.code, "SWITCH_ABORTED");
    return true;
  });
});
