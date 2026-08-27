import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import net from "node:net";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const gatewayDirectory = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const proxyKey = "gateway-test-key-material-at-least-32-bytes";

function listen(server) {
  return new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.off("error", reject);
      resolve(server.address().port);
    });
  });
}

async function reservePort() {
  const server = createServer();
  const port = await listen(server);
  await new Promise((resolve) => server.close(resolve));
  return port;
}

async function waitForGateway(baseUrl, child, diagnostics) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (child.exitCode != null) throw new Error(`Gateway arrêté: ${diagnostics()}`);
    try {
      const response = await fetch(`${baseUrl}/healthz`);
      if (response.ok) return;
    } catch {
      // Le processus démarre encore.
    }
    await new Promise((resolve) => setTimeout(resolve, 40));
  }
  throw new Error(`Gateway indisponible: ${diagnostics()}`);
}

function rawRequest(port, payload) {
  return new Promise((resolve, reject) => {
    const chunks = [];
    const socket = net.createConnection({ host: "127.0.0.1", port }, () => socket.end(payload));
    socket.on("data", (chunk) => chunks.push(chunk));
    socket.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    socket.on("error", reject);
  });
}

test("la façade loopback isole Social et conserve le trafic Switch", { timeout: 15_000 }, async (t) => {
  let observedSocialHeaders = null;
  const mainServer = createServer((request, response) => {
    response.writeHead(200, { "content-type": "text/plain; charset=utf-8" });
    response.end(request.url === "/healthz" ? "ok" : "main");
  });
  const socialServer = createServer((request, response) => {
    if (request.url === "/readyz") {
      response.writeHead(200, { "content-type": "application/json" });
      response.end('{"ok":true}');
      return;
    }
    observedSocialHeaders = request.headers;
    const reply = () => {
      response.writeHead(200, {
        "content-type": "text/plain; charset=utf-8",
        "set-cookie": "unexpected=secret; Path=/; HttpOnly",
      });
      response.end("social");
    };
    if (request.method === "POST" && request.url === "/api/social/sync") {
      setTimeout(reply, 250);
    } else reply();
  });
  const mainPort = await listen(mainServer);
  const socialPort = await listen(socialServer);
  const gatewayPort = await reservePort();
  let stderr = "";
  const child = spawn(process.execPath, [path.join(gatewayDirectory, "proxy.mjs")], {
    cwd: gatewayDirectory,
    env: {
      ...process.env,
      LISTEN_PORT: String(gatewayPort),
      MAIN_UPSTREAM_URL: `http://127.0.0.1:${mainPort}`,
      SOCIAL_UPSTREAM_URL: `http://127.0.0.1:${socialPort}`,
      SOCIAL_PROXY_KEY: proxyKey,
      SOCIAL_REQUEST_TIMEOUT_MS: "100",
      SOCIAL_SYNC_TIMEOUT_MS: "1000",
      SOCIAL_MAX_BODY_BYTES: "16",
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });

  t.after(async () => {
    if (child.exitCode == null) child.kill();
    await Promise.all([
      new Promise((resolve) => mainServer.close(resolve)),
      new Promise((resolve) => socialServer.close(resolve)),
    ]);
  });

  const baseUrl = `http://127.0.0.1:${gatewayPort}`;
  await waitForGateway(baseUrl, child, () => stderr);
  assert.equal(await (await fetch(`${baseUrl}/`)).text(), "main");

  const readiness = await (await fetch(`${baseUrl}/readyz`)).json();
  assert.deepEqual(readiness, { ok: true, mainReady: true, socialReady: true });
  const availability = await (await fetch(`${baseUrl}/api/social/availability`)).json();
  assert.deepEqual(availability, { ok: true, available: true });

  const social = await fetch(`${baseUrl}/social/`, {
    headers: {
      authorization: "Bearer client-forgery",
      cookie: "cst_session=browser-session",
      "x-social-proxy-key": "client-forgery",
    },
  });
  assert.equal(await social.text(), "social");
  assert.equal(observedSocialHeaders["x-social-proxy-key"], proxyKey);
  assert.equal(observedSocialHeaders.cookie, "cst_session=browser-session");
  assert.equal(observedSocialHeaders.authorization, undefined);
  assert.equal(social.headers.get("set-cookie"), null);

  const slowSync = await fetch(`${baseUrl}/api/social/sync`, { method: "POST" });
  assert.equal(slowSync.status, 200);
  assert.equal(await slowSync.text(), "social");

  const oversized = await fetch(`${baseUrl}/api/social/sync`, {
    method: "POST",
    body: "0123456789abcdefg",
  });
  assert.equal(oversized.status, 413);

  const malformed = await rawRequest(
    gatewayPort,
    "GET //[ HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
  );
  assert.match(malformed, /^HTTP\/1\.1 400/);
  assert.equal(child.exitCode, null);
});
