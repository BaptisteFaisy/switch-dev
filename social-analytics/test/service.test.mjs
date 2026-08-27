import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const serviceDirectory = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const proxyKey = "social-test-proxy-key-32-bytes-minimum";
const encryptionKey = "test-encryption-key-material-32-bytes-minimum";
const unsafeInstagramTokenUserId = Number.MAX_SAFE_INTEGER + 1;
const accountIds = {
  instagram: "11111111-1111-4111-8111-111111111111",
  demo: "22222222-2222-4222-8222-222222222222",
  foreign: "33333333-3333-4333-8333-333333333333",
};

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
  await new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve()));
  return port;
}

function sendJson(response, status, payload) {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(payload));
}

async function requestBody(request) {
  let body = "";
  for await (const chunk of request) body += chunk;
  return body;
}

async function waitUntilReady(baseUrl, child, diagnostics) {
  for (let attempt = 0; attempt < 100; attempt += 1) {
    if (child.exitCode != null) {
      throw new Error(`Le service a quitté prématurément (${child.exitCode}): ${diagnostics()}`);
    }
    try {
      const response = await fetch(`${baseUrl}/healthz`);
      if (response.ok) return;
    } catch {
      // Le processus démarre encore.
    }
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`Délai de démarrage dépassé: ${diagnostics()}`);
}

async function stopChild(child) {
  if (child.exitCode != null) return;
  await new Promise((resolve) => {
    const timeout = setTimeout(resolve, 1_500);
    child.once("exit", () => {
      clearTimeout(timeout);
      resolve();
    });
    child.kill();
  });
}

async function launchService(dataDirectory, providerPort, extraEnvironment = {}) {
  const servicePort = await reservePort();
  let stderr = "";
  const child = spawn(process.execPath, [path.join(serviceDirectory, "server.mjs")], {
    cwd: serviceDirectory,
    env: {
      ...process.env,
      PORT: String(servicePort),
      SOCIAL_DATA_DIR: dataDirectory,
      SOCIAL_TIMEZONE: "Europe/Paris",
      SOCIAL_DEMO_MODE: "false",
      SOCIAL_DEMO_SEED: "true",
      SOCIAL_INITIAL_SYNC_DAYS: "2",
      SOCIAL_SCHEDULED_SYNC_ENABLED: "false",
      SOCIAL_PROXY_KEY: proxyKey,
      SOCIAL_TOKEN_ENCRYPTION_KEY: encryptionKey,
      SOCIAL_PUBLIC_ORIGIN: `http://127.0.0.1:${servicePort}`,
      SWITCH_AUTH_URL: `http://127.0.0.1:${providerPort}/api/auth/me`,
      INSTAGRAM_CLIENT_ID: "ig-client",
      INSTAGRAM_CLIENT_SECRET: "ig-client-secret",
      INSTAGRAM_REDIRECT_URI: `http://127.0.0.1:${servicePort}/api/social/oauth/instagram/callback`,
      INSTAGRAM_AUTHORIZE_URL: `http://127.0.0.1:${providerPort}/instagram/authorize`,
      INSTAGRAM_TOKEN_URL: `http://127.0.0.1:${providerPort}/instagram/token`,
      INSTAGRAM_LONG_TOKEN_URL: `http://127.0.0.1:${providerPort}/instagram/long`,
      INSTAGRAM_GRAPH_URL: `http://127.0.0.1:${providerPort}/instagram/graph`,
      TIKTOK_CLIENT_KEY: "tt-client-key",
      TIKTOK_CLIENT_SECRET: "tt-client-secret",
      TIKTOK_REDIRECT_URI: `http://127.0.0.1:${servicePort}/api/social/oauth/tiktok/callback`,
      TIKTOK_AUTHORIZE_URL: `http://127.0.0.1:${providerPort}/tiktok/authorize`,
      TIKTOK_TOKEN_URL: `http://127.0.0.1:${providerPort}/tiktok/token`,
      TIKTOK_REFRESH_URL: `http://127.0.0.1:${providerPort}/tiktok/refresh`,
      TIKTOK_REVOKE_URL: `http://127.0.0.1:${providerPort}/tiktok/revoke`,
      TIKTOK_API_URL: `http://127.0.0.1:${providerPort}/tiktok/api`,
      ...extraEnvironment,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const baseUrl = `http://127.0.0.1:${servicePort}`;
  await waitUntilReady(baseUrl, child, () => stderr);

  async function api(pathname, { user = "user-a", headers = {}, ...options } = {}) {
    const requestHeaders = {
      "x-social-proxy-key": proxyKey,
      cookie: `cst_session=${user}`,
      ...headers,
    };
    const method = String(options.method ?? "GET").toUpperCase();
    if (method !== "GET" && method !== "HEAD" && method !== "OPTIONS") {
      requestHeaders.origin = baseUrl;
      if (requestHeaders["x-switch-social-request"] === "switch-development") {
        const configResponse = await fetch(`${baseUrl}/api/social/config`, {
          headers: {
            "x-social-proxy-key": proxyKey,
            cookie: `cst_session=${user}`,
          },
        });
        const config = await configResponse.json();
        requestHeaders["x-switch-social-request"] = config.csrfToken;
      }
    }
    const response = await fetch(`${baseUrl}${pathname}`, {
      ...options,
      headers: requestHeaders,
    });
    const payload = await response.json();
    return { response, payload };
  }

  return { baseUrl, child, api, diagnostics: () => stderr };
}

async function connectProvider(service, platform, code) {
  const started = await service.api(`/api/social/oauth/${platform}/start`, {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
    body: "{}",
  });
  assert.equal(started.response.status, 200);
  const authorizeUrl = new URL(started.payload.authorizeUrl);
  assert.ok(authorizeUrl.searchParams.get("state"));
  const callback = await fetch(
    `${service.baseUrl}/api/social/oauth/${platform}/callback?code=${encodeURIComponent(code)}&state=${encodeURIComponent(authorizeUrl.searchParams.get("state"))}`,
    {
      redirect: "manual",
      headers: {
        "x-social-proxy-key": proxyKey,
        cookie: "cst_session=user-a",
      },
    },
  );
  return { started, authorizeUrl, callback };
}

test("OAuth canonique, médias exacts, migration v1 et isolation propriétaire", { timeout: 90_000 }, async (t) => {
  const dataDirectory = await mkdtemp(path.join(tmpdir(), "switch-social-v2-test-"));
  const storePath = path.join(dataDirectory, "social-store.json");
  const legacyStore = {
    version: 1,
    accounts: [
      {
        id: accountIds.instagram,
        ownerUserId: "user-a",
        platform: "instagram",
        externalAccountId: "ig-app-scoped",
        handle: "ancien.pseudo",
        displayName: "Ancien compte",
        colour: "#123456",
        connectionMode: "live",
        status: "error",
        createdAt: "2026-01-01T00:00:00.000Z",
        syncedAt: null,
      },
      {
        id: accountIds.demo,
        ownerUserId: "user-a",
        platform: "tiktok",
        handle: "demo.cache",
        displayName: "Démo conservée",
        colour: "#654321",
        connectionMode: "demo",
        status: "connected",
        createdAt: "2026-01-01T00:00:00.000Z",
        syncedAt: "2026-01-01T00:00:00.000Z",
      },
      {
        id: accountIds.foreign,
        ownerUserId: "user-b",
        platform: "instagram",
        externalAccountId: "foreign-canonical",
        handle: "foreign.owner",
        displayName: "Foreign Owner",
        colour: "#abcdef",
        connectionMode: "live",
        status: "connected",
        createdAt: "2026-01-01T00:00:00.000Z",
        syncedAt: "2026-01-01T00:00:00.000Z",
      },
    ],
    metrics: [
      {
        accountId: accountIds.instagram,
        ownerUserId: "user-a",
        date: "2026-01-01",
        views: 17,
        source: "live",
      },
      {
        accountId: accountIds.demo,
        ownerUserId: "user-a",
        date: "2026-01-01",
        views: 999999,
        source: "demo",
      },
      {
        accountId: accountIds.instagram,
        ownerUserId: "user-b",
        date: "2026-01-02",
        views: 88,
        source: "live",
      },
    ],
    oauthAttempts: [],
    demoSeededOwners: ["user-a"],
    legacyMarker: { preserved: true },
  };
  await writeFile(storePath, JSON.stringify(legacyStore), "utf8");

  const observations = {
    instagramTokenForms: [],
    instagramProfileFields: [],
    instagramMediaRequests: [],
    tiktokTokenForms: [],
    tiktokRefreshForms: [],
    tiktokProfileFields: [],
    tiktokVideoBodies: [],
    tiktokVideoFields: [],
    revokedForms: [],
    providerFailures: [],
  };
  let tiktokPaginationMode = "complete";
  let instagramInsightsUnavailable = false;
  let instagramInsightsMissingMetric = false;
  let instagramMediaUnavailable = false;

  const providerServer = createServer((request, response) => {
    void (async () => {
      const requestUrl = new URL(request.url, "http://test.local");
      const session = request.headers.cookie?.match(/(?:^|;\s*)cst_session=([^;]+)/)?.[1];
      if (requestUrl.pathname === "/api/auth/me") {
        if (!session) {
          sendJson(response, 401, { ok: false });
        } else {
          sendJson(response, 200, { user: { id: session, username: session } });
        }
        return;
      }
      if (requestUrl.pathname === "/instagram/token") {
        assert.match(request.headers["content-type"], /^multipart\/form-data; boundary=/);
        const rawBody = await requestBody(request);
        const form = Object.fromEntries(
          [...rawBody.matchAll(/name="([^"]+)"\r\n\r\n([^\r\n]*)/g)]
            .map((match) => [match[1], match[2]]),
        );
        observations.instagramTokenForms.push(form);
        sendJson(response, 200, {
          data: [{
            access_token: "ig-short-secret",
            user_id: unsafeInstagramTokenUserId,
            permissions: ["instagram_business_basic", "instagram_business_manage_insights"],
          }],
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/long") {
        assert.equal(requestUrl.searchParams.get("access_token"), "ig-short-secret");
        sendJson(response, 200, {
          access_token: "ig-long-secret",
          expires_in: 5_184_000,
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/me") {
        observations.instagramProfileFields.push(requestUrl.searchParams.get("fields"));
        assert.equal(request.headers.authorization, "Bearer ig-long-secret");
        sendJson(response, 200, {
          id: "ig-app-scoped",
          user_id: "ig-canonical",
          username: "live.instagram",
          name: "Live Instagram",
          account_type: "BUSINESS",
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/ig-canonical/insights") {
        if (instagramInsightsUnavailable) {
          sendJson(response, 503, { error: { message: "insights unavailable" } });
          return;
        }
        if (instagramInsightsMissingMetric) {
          sendJson(response, 200, { data: [] });
          return;
        }
        assert.equal(requestUrl.searchParams.get("metric"), "views");
        assert.match(requestUrl.searchParams.get("since"), /^\d{10}$/);
        assert.match(requestUrl.searchParams.get("until"), /^\d{10}$/);
        assert.equal(
          Number(requestUrl.searchParams.get("until"))
            - Number(requestUrl.searchParams.get("since")),
          86_399,
        );
        sendJson(response, 200, {
          data: [{ name: "views", period: "day", total_value: { value: 321 } }],
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/ig-canonical/media") {
        if (instagramMediaUnavailable) {
          sendJson(response, 503, { error: { message: "media unavailable" } });
          return;
        }
        const after = requestUrl.searchParams.get("after");
        observations.instagramMediaRequests.push({
          after,
          fields: requestUrl.searchParams.get("fields"),
        });
        if (!after) {
          sendJson(response, 200, {
            data: [
              {
                id: "ig-reel-1",
                caption: "Premier Reel",
                media_type: "VIDEO",
                is_shared_to_feed: true,
                permalink: "https://www.instagram.com/reel/one/",
                thumbnail_url: "https://cdn.example/one.jpg",
                timestamp: "2026-08-20T10:00:00Z",
              },
              {
                id: "ig-regular-video",
                caption: "Vidéo feed",
                media_type: "VIDEO",
                permalink: "https://www.instagram.com/p/feed/",
                timestamp: "2026-08-20T11:00:00Z",
              },
              {
                id: "ig-reel-failed",
                caption: "Bad\n reel\u0000",
                media_type: "VIDEO",
                is_shared_to_feed: false,
                permalink: "javascript:alert(1)",
                timestamp: "2026-08-21T10:00:00Z",
              },
            ],
            paging: { cursors: { after: "ig-page-2" } },
          });
        } else {
          assert.equal(after, "ig-page-2");
          sendJson(response, 200, {
            data: [
              {
                id: "ig-reel-2",
                caption: "Deuxième Reel",
                media_type: "VIDEO",
                is_shared_to_feed: false,
                permalink: "https://www.instagram.com/reel/two/",
                timestamp: "2026-08-22T10:00:00Z",
              },
              {
                id: "ig-carousel",
                caption: "Pas un Reel",
                media_type: "CAROUSEL_ALBUM",
                permalink: "https://www.instagram.com/p/carousel/",
                timestamp: "2026-08-22T11:00:00Z",
              },
            ],
            paging: { cursors: { after: "ig-page-2" } },
          });
        }
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/ig-reel-1/insights") {
        sendJson(response, 200, {
          data: [{ name: "views", total_value: { value: 1_234 } }],
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/ig-reel-2/insights") {
        sendJson(response, 200, {
          data: [{ name: "views", values: [{ value: 567 }] }],
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/graph/ig-reel-failed/insights") {
        sendJson(response, 503, { error: { message: "metric temporarily unavailable" } });
        return;
      }
      if (requestUrl.pathname === "/tiktok/token") {
        assert.match(request.headers["content-type"], /^application\/x-www-form-urlencoded/);
        const form = Object.fromEntries(new URLSearchParams(await requestBody(request)));
        observations.tiktokTokenForms.push(form);
        sendJson(response, 200, {
          access_token: "tt-access-secret",
          expires_in: 1,
          refresh_token: "tt-refresh-secret",
          refresh_expires_in: 31_536_000,
          open_id: "tt-stable",
          scope: "user.info.basic,user.info.profile,video.list",
          token_type: "Bearer",
        });
        return;
      }
      if (requestUrl.pathname === "/tiktok/refresh") {
        assert.match(request.headers["content-type"], /^application\/x-www-form-urlencoded/);
        const form = Object.fromEntries(new URLSearchParams(await requestBody(request)));
        observations.tiktokRefreshForms.push(form);
        sendJson(response, 200, {
          access_token: "tt-refreshed-secret",
          expires_in: 86_400,
          refresh_token: "tt-refresh-rotated-secret",
          refresh_expires_in: 31_536_000,
          open_id: "tt-stable",
          scope: "user.info.basic,user.info.profile,video.list",
          token_type: "Bearer",
        });
        return;
      }
      if (requestUrl.pathname === "/tiktok/revoke") {
        assert.match(request.headers["content-type"], /^application\/x-www-form-urlencoded/);
        const form = Object.fromEntries(new URLSearchParams(await requestBody(request)));
        observations.revokedForms.push(form);
        response.writeHead(200);
        response.end();
        return;
      }
      if (requestUrl.pathname === "/tiktok/api/user/info/") {
        observations.tiktokProfileFields.push(requestUrl.searchParams.get("fields"));
        assert.equal(request.headers.authorization, "Bearer tt-access-secret");
        sendJson(response, 200, {
          data: {
            user: {
              open_id: "tt-stable",
              union_id: "tt-union",
              username: "live.tiktok",
              display_name: "Live TikTok",
            },
          },
          error: { code: "ok", message: "", log_id: "profile-log" },
        });
        return;
      }
      if (requestUrl.pathname === "/tiktok/api/video/list/") {
        assert.equal(request.method, "POST");
        assert.match(request.headers.authorization, /^Bearer tt-(?:access|refreshed)-secret$/);
        observations.tiktokVideoFields.push(requestUrl.searchParams.get("fields"));
        const body = JSON.parse(await requestBody(request));
        observations.tiktokVideoBodies.push(body);
        if (body.cursor == null) {
          sendJson(response, 200, {
            data: {
              videos: [
                {
                  id: "tt-video-1",
                  create_time: 1_724_000_000,
                  title: "Première TikTok",
                  share_url: "https://www.tiktok.com/@live/video/1",
                  cover_image_url: "https://cdn.example/tt-one.jpg",
                  view_count: 4_444,
                },
                ...(tiktokPaginationMode === "null_view_complete" ? [{
                  id: "tt-video-null",
                  create_time: 1_724_200_000,
                  title: "Compteur indisponible",
                  share_url: "https://www.tiktok.com/@live/video/null",
                  view_count: null,
                }] : []),
              ],
              has_more: !["one_video_complete", "null_view_complete"].includes(tiktokPaginationMode),
              cursor: 777,
            },
            error: { code: "ok", message: "", log_id: "video-page-1" },
          });
        } else {
          assert.equal(body.cursor, 777);
          sendJson(response, 200, {
            data: {
              videos: [{
                id: "tt-video-2",
                create_time: 1_724_100_000,
                video_description: "Deuxième TikTok",
                share_url: "https://www.tiktok.com/@live/video/2",
                cover_image_url: "https://cdn.example/tt-two.jpg",
                view_count: 99,
              }],
              has_more: tiktokPaginationMode === "partial",
              cursor: 777,
            },
            error: { code: "ok", message: "", log_id: "video-page-2" },
          });
        }
        return;
      }
      sendJson(response, 404, { error: { message: "not found" } });
    })().catch((error) => {
      observations.providerFailures.push(error);
      if (!response.headersSent) sendJson(response, 500, { error: { message: error.message } });
      else response.end();
    });
  });

  const providerPort = await listen(providerServer);
  const service = await launchService(dataDirectory, providerPort);
  t.after(async () => {
    await stopChild(service.child);
    await new Promise((resolve) => providerServer.close(() => resolve()));
    await rm(dataDirectory, { recursive: true, force: true });
  });

  const directStatic = await fetch(`${service.baseUrl}/social/`);
  assert.equal(directStatic.status, 403);
  const noSession = await fetch(`${service.baseUrl}/api/social/accounts`, {
    headers: { "x-social-proxy-key": proxyKey },
  });
  assert.equal(noSession.status, 401);

  const csrfConfig = await service.api("/api/social/config");
  assert.match(csrfConfig.payload.csrfToken, /^[A-Za-z0-9_-]{40,}$/);
  const otherUserConfig = await service.api("/api/social/config", { user: "user-b" });
  assert.notEqual(csrfConfig.payload.csrfToken, otherUserConfig.payload.csrfToken);
  const missingCsrf = await fetch(`${service.baseUrl}/api/social/sync`, {
    method: "POST",
    headers: {
      "x-social-proxy-key": proxyKey,
      cookie: "cst_session=user-a",
      origin: service.baseUrl,
    },
  });
  assert.equal(missingCsrf.status, 403);
  const foreignOrigin = await fetch(`${service.baseUrl}/api/social/sync`, {
    method: "POST",
    headers: {
      "x-social-proxy-key": proxyKey,
      "x-switch-social-request": csrfConfig.payload.csrfToken,
      cookie: "cst_session=user-a",
      origin: "https://example.invalid",
    },
  });
  assert.equal(foreignOrigin.status, 403);

  const initialAccounts = await service.api("/api/social/accounts");
  assert.deepEqual(initialAccounts.payload.accounts.map((account) => account.id), [accountIds.instagram]);
  const initialMetrics = await service.api("/api/social/metrics?range=7");
  assert.deepEqual(initialMetrics.payload.series.map((series) => series.account.id), [accountIds.instagram]);
  const migratedBeforeOauth = JSON.parse(await readFile(storePath, "utf8"));
  assert.equal(migratedBeforeOauth.version, 2);
  assert.deepEqual(migratedBeforeOauth.media, []);
  assert.deepEqual(migratedBeforeOauth.mediaSnapshots, []);
  assert.equal(migratedBeforeOauth.accounts.length, 3);
  assert.equal(migratedBeforeOauth.metrics.length, 3);
  assert.deepEqual(migratedBeforeOauth.legacyMarker, { preserved: true });
  assert.equal(Number.isSafeInteger(unsafeInstagramTokenUserId), false);

  const instagramConnection = await connectProvider(service, "instagram", "ig-code");
  assert.equal(
    instagramConnection.callback.headers.get("location"),
    "/?switch_social=connected&provider=instagram#switch-social",
  );
  assert.equal(instagramConnection.authorizeUrl.searchParams.get("client_id"), "ig-client");
  assert.equal(instagramConnection.authorizeUrl.searchParams.get("enable_fb_login"), "0");
  assert.equal(instagramConnection.authorizeUrl.searchParams.get("force_authentication"), "1");

  const tiktokConnection = await connectProvider(service, "tiktok", "tt-code");
  assert.equal(
    tiktokConnection.callback.headers.get("location"),
    "/?switch_social=connected&provider=tiktok#switch-social",
  );
  assert.equal(tiktokConnection.authorizeUrl.searchParams.get("client_key"), "tt-client-key");
  assert.equal(tiktokConnection.authorizeUrl.searchParams.has("app_id"), false);
  assert.equal(
    tiktokConnection.authorizeUrl.searchParams.get("scope"),
    "user.info.basic,user.info.profile,video.list",
  );

  const sync = await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.equal(sync.response.status, 200, service.diagnostics());
  assert.equal(sync.payload.partial, true);
  assert.equal(sync.payload.failedAccounts, 0);
  assert.ok(sync.payload.partialAccounts >= 1);

  const connectedAccounts = await service.api("/api/social/accounts");
  assert.equal(connectedAccounts.payload.accounts.length, 2);
  assert.equal(
    connectedAccounts.payload.accounts.filter((account) => account.platform === "instagram").length,
    1,
  );
  const instagramAccount = connectedAccounts.payload.accounts.find((account) => account.platform === "instagram");
  const tiktokAccount = connectedAccounts.payload.accounts.find((account) => account.platform === "tiktok");
  assert.equal(instagramAccount.id, accountIds.instagram);
  assert.equal(instagramAccount.handle, "live.instagram");
  assert.ok(tiktokAccount);
  assert.equal(tiktokAccount.handle, "live.tiktok");
  assert.equal(tiktokAccount.displayName, "Live TikTok");

  let persisted = JSON.parse(await readFile(storePath, "utf8"));
  const persistedInstagram = persisted.accounts.find((account) => account.id === accountIds.instagram);
  assert.equal(persistedInstagram.externalAccountId, "ig-canonical");
  assert.equal(persistedInstagram.appScopedAccountId, "ig-app-scoped");
  assert.equal(
    persisted.metrics.some((metric) =>
      metric.accountId === accountIds.instagram
        && metric.date === "2026-01-01"
        && metric.views === 17
    ),
    false,
    "les métriques liées à l'ancien identifiant Instagram doivent être purgées",
  );
  assert.equal(
    persisted.metrics.some((metric) =>
      metric.accountId === accountIds.instagram
        && metric.ownerUserId === "user-b"
        && metric.date === "2026-01-02"
        && metric.views === 88
    ),
    true,
    "la purge d'identité ne doit jamais supprimer les données d'un autre propriétaire",
  );
  const persistedTikTok = persisted.accounts.find((account) => account.id === tiktokAccount.id);
  assert.equal(persistedTikTok.externalAccountId, "tt-stable");
  assert.equal(persistedTikTok.unionId, "tt-union");

  const initialMedia = await service.api("/api/social/media?range=7");
  const initialByExternalId = new Map(
    initialMedia.payload.items.map((item) => [item.externalMediaId, item]),
  );
  assert.equal(initialMedia.payload.items.length, 5);
  assert.equal(initialByExternalId.get("ig-reel-1").latestViews, 1_234);
  assert.equal(initialByExternalId.get("ig-reel-2").latestViews, 567);
  assert.equal(initialByExternalId.get("ig-reel-failed").latestViews, null);
  assert.equal(initialByExternalId.get("ig-reel-failed").title, "Bad reel");
  assert.equal(initialByExternalId.get("ig-reel-failed").permalink, null);
  assert.equal(initialByExternalId.get("tt-video-1").latestViews, 4_444);
  assert.equal(initialByExternalId.get("tt-video-2").latestViews, 99);
  assert.equal(initialByExternalId.get("tt-video-1").periodViews, null);
  assert.equal(initialByExternalId.get("tt-video-2").periodViews, null);
  assert.equal(initialByExternalId.has("ig-regular-video"), false);
  assert.equal(initialByExternalId.has("ig-carousel"), false);
  assert.ok(
    initialByExternalId.get("tt-video-1").points.every(
      (point) => point.available === false && point.views === null,
    ),
  );

  const today = initialMedia.payload.dates.at(-1);
  const yesterday = initialMedia.payload.dates.at(-2);
  persisted = JSON.parse(await readFile(storePath, "utf8"));
  const tiktokMedia = persisted.media.filter((media) => media.accountId === tiktokAccount.id);
  const baselineViews = new Map([
    ["tt-video-1", 4_400],
    ["tt-video-2", 80],
  ]);
  for (const media of tiktokMedia) {
    const currentSnapshot = persisted.mediaSnapshots.find((snapshot) =>
      snapshot.mediaId === media.id && snapshot.date === today
    );
    assert.ok(currentSnapshot?.capturedAt);
    persisted.mediaSnapshots.push({
      id: `baseline-${media.id}`,
      ownerUserId: "user-a",
      accountId: tiktokAccount.id,
      mediaId: media.id,
      date: yesterday,
      views: baselineViews.get(media.externalMediaId),
      capturedAt: new Date(Date.parse(currentSnapshot.capturedAt) - 86_400_000).toISOString(),
      source: "live",
    });
  }
  persisted.media.push({
    id: "foreign-media",
    ownerUserId: "user-b",
    accountId: accountIds.foreign,
    platform: "instagram",
    externalMediaId: "foreign-reel",
    title: "Foreign reel",
    permalink: "https://www.instagram.com/reel/foreign/",
    publishedAt: "2026-08-23T10:00:00.000Z",
    createdAt: "2026-08-23T10:00:00.000Z",
    updatedAt: "2026-08-23T10:00:00.000Z",
  });
  persisted.mediaSnapshots.push({
    id: "foreign-snapshot",
    ownerUserId: "user-b",
    accountId: accountIds.foreign,
    mediaId: "foreign-media",
    date: today,
    views: 12,
    capturedAt: `${today}T20:00:00.000Z`,
    source: "live",
  });
  await writeFile(storePath, JSON.stringify(persisted), "utf8");

  await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  const mediaWithDeltas = await service.api("/api/social/media?range=7");
  const withDeltaByExternalId = new Map(
    mediaWithDeltas.payload.items.map((item) => [item.externalMediaId, item]),
  );
  const tiktokOne = withDeltaByExternalId.get("tt-video-1");
  const tiktokTwo = withDeltaByExternalId.get("tt-video-2");
  assert.equal(tiktokOne.points.find((point) => point.date === today).views, 44);
  assert.equal(tiktokTwo.points.find((point) => point.date === today).views, 19);
  assert.equal(tiktokOne.periodViews, 44);
  assert.equal(tiktokTwo.periodViews, 19);
  assert.equal(tiktokOne.periodComplete, false);
  assert.equal(mediaWithDeltas.payload.missingPoints > 0, true);

  persisted = JSON.parse(await readFile(storePath, "utf8"));
  const regressingMedia = persisted.media.find((media) => media.externalMediaId === "tt-video-1");
  const regressingSnapshot = persisted.mediaSnapshots.find((snapshot) =>
    snapshot.mediaId === regressingMedia.id && snapshot.date === today
  );
  regressingSnapshot.views = 4_390;
  await writeFile(storePath, JSON.stringify(persisted), "utf8");
  const mediaWithRegressingCounter = await service.api("/api/social/media?range=7");
  const regressingItem = mediaWithRegressingCounter.payload.items.find(
    (item) => item.externalMediaId === "tt-video-1",
  );
  const regressingPoint = regressingItem.points.find((point) => point.date === today);
  assert.equal(regressingPoint.available, false);
  assert.equal(regressingPoint.views, null);
  assert.equal(regressingItem.periodViews, null);
  assert.equal(regressingItem.periodComplete, false);
  regressingSnapshot.views = 4_444;
  await writeFile(storePath, JSON.stringify(persisted), "utf8");

  const filteredMedia = await service.api(
    `/api/social/media?range=30&accounts=${encodeURIComponent(tiktokAccount.id)}`,
  );
  assert.deepEqual(
    new Set(filteredMedia.payload.items.map((item) => item.accountId)),
    new Set([tiktokAccount.id]),
  );
  const foreignMedia = await service.api("/api/social/media?range=7", { user: "user-b" });
  assert.deepEqual(foreignMedia.payload.items.map((item) => item.externalMediaId), ["foreign-reel"]);
  const forbiddenForeignFilter = await service.api(
    `/api/social/media?range=7&accounts=${encodeURIComponent(tiktokAccount.id)}`,
    { user: "user-b" },
  );
  assert.deepEqual(forbiddenForeignFilter.payload.items, []);

  const accountMetrics = await service.api("/api/social/metrics?range=7");
  const tiktokSeries = accountMetrics.payload.series.find(
    (series) => series.account.id === tiktokAccount.id,
  );
  assert.equal(tiktokSeries.points.find((point) => point.date === today).views, 63);
  assert.equal(tiktokSeries.points.find((point) => point.date === yesterday).available, false);

  tiktokPaginationMode = "partial";
  const instagramRequestsBeforePartial = observations.instagramMediaRequests.length;
  const tiktokRequestsBeforePartial = observations.tiktokVideoBodies.length;
  const partialTikTokSync = await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.equal(partialTikTokSync.payload.partial, true);
  assert.equal(
    observations.instagramMediaRequests.length - instagramRequestsBeforePartial,
    2,
    "la garde Instagram doit arrêter le curseur répété après deux pages",
  );
  assert.equal(
    observations.tiktokVideoBodies.length - tiktokRequestsBeforePartial,
    2,
    "la garde TikTok doit arrêter le curseur répété après deux pages",
  );
  const metricsAfterPartialTikTok = await service.api("/api/social/metrics?range=7");
  const tiktokAfterPartial = metricsAfterPartialTikTok.payload.series.find(
    (series) => series.account.id === tiktokAccount.id,
  );
  assert.equal(
    tiktokAfterPartial.points.find((point) => point.date === today).available,
    false,
    "une pagination TikTok incomplète ne doit jamais publier un total partiel",
  );

  tiktokPaginationMode = "one_video_complete";
  await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  const metricsAfterVideoDisappeared = await service.api("/api/social/metrics?range=7");
  const tiktokAfterVideoDisappeared = metricsAfterVideoDisappeared.payload.series.find(
    (series) => series.account.id === tiktokAccount.id,
  );
  assert.equal(
    tiktokAfterVideoDisappeared.points.find((point) => point.date === today).views,
    44,
    "une collecte complète ne doit pas réutiliser une vidéo absente du batch courant",
  );

  tiktokPaginationMode = "null_view_complete";
  await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  const mediaWithNullCounter = await service.api("/api/social/media?range=7");
  assert.equal(
    mediaWithNullCounter.payload.items.find(
      (item) => item.externalMediaId === "tt-video-null",
    ).latestViews,
    null,
    "un compteur API null doit rester indisponible et ne jamais devenir zéro",
  );

  tiktokPaginationMode = "one_video_complete";
  const beforeMissingInstagramMetric = JSON.parse(await readFile(storePath, "utf8"))
    .accounts.find((account) => account.id === accountIds.instagram);
  assert.ok(beforeMissingInstagramMetric.syncedAt);
  instagramInsightsMissingMetric = true;
  const missingInstagramMetricSync = await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.equal(missingInstagramMetricSync.payload.partial, true);
  const afterMissingInstagramMetric = JSON.parse(await readFile(storePath, "utf8"))
    .accounts.find((account) => account.id === accountIds.instagram);
  assert.equal(afterMissingInstagramMetric.syncedAt, beforeMissingInstagramMetric.syncedAt);
  instagramInsightsMissingMetric = false;

  const beforeInstagramFailure = JSON.parse(await readFile(storePath, "utf8"))
    .accounts.find((account) => account.id === accountIds.instagram);
  instagramInsightsUnavailable = true;
  instagramMediaUnavailable = true;
  const failedInstagramSync = await service.api("/api/social/sync", {
    method: "POST",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.ok(failedInstagramSync.payload.failedAccounts >= 1);
  const afterInstagramFailure = JSON.parse(await readFile(storePath, "utf8"))
    .accounts.find((account) => account.id === accountIds.instagram);
  assert.equal(afterInstagramFailure.status, "error");
  assert.equal(afterInstagramFailure.syncedAt, beforeInstagramFailure.syncedAt);
  instagramInsightsUnavailable = false;
  instagramMediaUnavailable = false;

  const forbiddenDelete = await service.api(`/api/social/accounts/${tiktokAccount.id}`, {
    user: "user-b",
    method: "DELETE",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.equal(forbiddenDelete.response.status, 404);
  const deleted = await service.api(`/api/social/accounts/${tiktokAccount.id}`, {
    method: "DELETE",
    headers: { "x-switch-social-request": "switch-development" },
  });
  assert.equal(deleted.response.status, 200);

  persisted = JSON.parse(await readFile(storePath, "utf8"));
  assert.equal(persisted.version, 2);
  assert.ok(persisted.accounts.some((account) => account.id === accountIds.demo));
  assert.ok(persisted.metrics.some((metric) => metric.accountId === accountIds.demo && metric.views === 999999));
  assert.ok(!persisted.accounts.some((account) => account.id === tiktokAccount.id));
  assert.ok(!persisted.media.some((media) => media.accountId === tiktokAccount.id));
  assert.ok(!persisted.mediaSnapshots.some((snapshot) => snapshot.accountId === tiktokAccount.id));
  const serializedStore = JSON.stringify(persisted);
  for (const secret of [
    "ig-short-secret",
    "ig-long-secret",
    "tt-access-secret",
    "tt-refreshed-secret",
    "tt-refresh-secret",
    "tt-refresh-rotated-secret",
    "ig-client-secret",
    "tt-client-secret",
  ]) {
    assert.equal(serializedStore.includes(secret), false, `secret en clair: ${secret}`);
  }

  assert.deepEqual(observations.instagramTokenForms.map((form) => form.code), ["ig-code"]);
  assert.ok(observations.instagramProfileFields.every((fields) => fields.includes("id,user_id")));
  assert.ok(observations.instagramMediaRequests.every((entry) => !entry.fields.includes("media_product_type")));
  assert.ok(observations.instagramMediaRequests.every((entry) => entry.fields.includes("is_shared_to_feed")));
  assert.deepEqual(observations.tiktokTokenForms.map((form) => form.client_key), ["tt-client-key"]);
  assert.deepEqual(observations.tiktokTokenForms.map((form) => form.code), ["tt-code"]);
  assert.ok(observations.tiktokTokenForms.every((form) => !("auth_code" in form) && !("client_id" in form)));
  assert.ok(observations.tiktokRefreshForms.length >= 1);
  assert.ok(observations.tiktokRefreshForms.every((form) => form.grant_type === "refresh_token"));
  assert.ok(observations.tiktokRefreshForms.every((form) => form.client_key === "tt-client-key"));
  assert.ok(observations.tiktokProfileFields.every((fields) => fields === "open_id,union_id,username,display_name,avatar_url"));
  assert.ok(observations.tiktokVideoFields.every((fields) => fields.split(",").includes("view_count")));
  assert.ok(observations.tiktokVideoBodies.every((body) => body.max_count === 20));
  assert.deepEqual(observations.revokedForms, [{
    client_key: "tt-client-key",
    client_secret: "tt-client-secret",
    token: "tt-refreshed-secret",
  }]);
  assert.deepEqual(observations.providerFailures, []);
});

test("TikTok refuse une identité incohérente ou sans username exact", { timeout: 45_000 }, async (t) => {
  const dataDirectory = await mkdtemp(path.join(tmpdir(), "switch-social-mismatch-test-"));
  const tokenForms = [];
  let profileMode = "mismatch";
  const providerServer = createServer((request, response) => {
    void (async () => {
      const requestUrl = new URL(request.url, "http://test.local");
      const session = request.headers.cookie?.match(/(?:^|;\s*)cst_session=([^;]+)/)?.[1];
      if (requestUrl.pathname === "/api/auth/me") {
        sendJson(response, session ? 200 : 401, session
          ? { user: { id: session, username: session } }
          : { ok: false });
        return;
      }
      if (requestUrl.pathname === "/tiktok/token") {
        const form = Object.fromEntries(new URLSearchParams(await requestBody(request)));
        tokenForms.push(form);
        sendJson(response, 200, {
          access_token: "mismatch-access-secret",
          expires_in: 86_400,
          refresh_token: "mismatch-refresh-secret",
          refresh_expires_in: 31_536_000,
          open_id: "token-open-id",
          scope: "user.info.basic,user.info.profile,video.list",
          token_type: "Bearer",
        });
        return;
      }
      if (requestUrl.pathname === "/tiktok/api/user/info/") {
        sendJson(response, 200, {
          data: {
            user: profileMode === "mismatch"
              ? {
                open_id: "different-profile-open-id",
                username: "wrong.identity",
                display_name: "Wrong identity",
              }
              : {
                open_id: "token-open-id",
                display_name: "Missing username",
              },
          },
          error: { code: "ok", message: "", log_id: "mismatch-log" },
        });
        return;
      }
      sendJson(response, 404, { error: { code: "not_found", message: "not found" } });
    })();
  });
  const providerPort = await listen(providerServer);
  const service = await launchService(dataDirectory, providerPort);
  t.after(async () => {
    await stopChild(service.child);
    await new Promise((resolve) => providerServer.close(() => resolve()));
    await rm(dataDirectory, { recursive: true, force: true });
  });

  const connection = await connectProvider(service, "tiktok", "mismatch-code");
  assert.equal(
    connection.callback.headers.get("location"),
    "/?switch_social=error&provider=tiktok#switch-social",
  );
  const accounts = await service.api("/api/social/accounts");
  assert.deepEqual(accounts.payload.accounts, []);

  profileMode = "missing-username";
  const missingUsername = await connectProvider(service, "tiktok", "missing-username-code");
  assert.equal(
    missingUsername.callback.headers.get("location"),
    "/?switch_social=error&provider=tiktok#switch-social",
  );
  const accountsAfterMissingUsername = await service.api("/api/social/accounts");
  assert.deepEqual(accountsAfterMissingUsername.payload.accounts, []);

  const persisted = await readFile(path.join(dataDirectory, "social-store.json"), "utf8");
  assert.equal(JSON.parse(persisted).version, 2);
  assert.equal(persisted.includes("mismatch-access-secret"), false);
  assert.equal(persisted.includes("mismatch-refresh-secret"), false);
  assert.deepEqual(tokenForms.map((form) => form.client_key), ["tt-client-key", "tt-client-key"]);
});

test("OAuth refuse les jetons incomplets ou sans permissions minimales", { timeout: 45_000 }, async (t) => {
  const dataDirectory = await mkdtemp(path.join(tmpdir(), "switch-social-scope-test-"));
  let profileRequests = 0;
  let instagramLongRequests = 0;
  let instagramTokenMode = "limited-scope";
  let tiktokTokenMode = "limited-scope";
  const providerServer = createServer((request, response) => {
    void (async () => {
      const requestUrl = new URL(request.url, "http://test.local");
      const session = request.headers.cookie?.match(/(?:^|;\s*)cst_session=([^;]+)/)?.[1];
      if (requestUrl.pathname === "/api/auth/me") {
        sendJson(response, session ? 200 : 401, session
          ? { user: { id: session, username: session } }
          : { ok: false });
        return;
      }
      if (requestUrl.pathname === "/instagram/token") {
        await requestBody(request);
        sendJson(response, 200, {
          data: [{
            access_token: "limited-instagram-secret",
            user_id: "limited-instagram-id",
            permissions: instagramTokenMode === "limited-scope"
              ? ["instagram_business_basic"]
              : ["instagram_business_basic", "instagram_business_manage_insights"],
          }],
        });
        return;
      }
      if (requestUrl.pathname === "/instagram/long") {
        instagramLongRequests += 1;
        sendJson(response, 200, instagramTokenMode === "zero-expiry"
          ? { access_token: "invalid-long-instagram-secret", expires_in: 0 }
          : {});
        return;
      }
      if (requestUrl.pathname === "/tiktok/token") {
        await requestBody(request);
        sendJson(response, 200, {
          access_token: "limited-access-secret",
          expires_in: 86_400,
          ...(tiktokTokenMode === "limited-scope"
            ? { refresh_token: "limited-refresh-secret" }
            : {}),
          refresh_expires_in: 31_536_000,
          open_id: "limited-open-id",
          scope: tiktokTokenMode === "limited-scope"
            ? "user.info.basic"
            : "user.info.basic,user.info.profile,video.list",
          token_type: "Bearer",
        });
        return;
      }
      if (
        requestUrl.pathname.startsWith("/instagram/graph/")
        || requestUrl.pathname === "/tiktok/api/user/info/"
      ) {
        profileRequests += 1;
      }
      sendJson(response, 404, { error: { code: "not_found", message: "not found" } });
    })();
  });
  const providerPort = await listen(providerServer);
  const service = await launchService(dataDirectory, providerPort);
  t.after(async () => {
    await stopChild(service.child);
    await new Promise((resolve) => providerServer.close(() => resolve()));
    await rm(dataDirectory, { recursive: true, force: true });
  });

  const connection = await connectProvider(service, "tiktok", "limited-scope-code");
  assert.equal(
    connection.callback.headers.get("location"),
    "/?switch_social=error&provider=tiktok#switch-social",
  );
  const accounts = await service.api("/api/social/accounts");
  assert.deepEqual(accounts.payload.accounts, []);
  assert.equal(profileRequests, 0);
  const instagramConnection = await connectProvider(service, "instagram", "limited-scope-code");
  assert.equal(
    instagramConnection.callback.headers.get("location"),
    "/?switch_social=error&provider=instagram#switch-social",
  );
  const accountsAfterInstagram = await service.api("/api/social/accounts");
  assert.deepEqual(accountsAfterInstagram.payload.accounts, []);
  assert.equal(profileRequests, 0);

  tiktokTokenMode = "missing-refresh";
  const incompleteTikTok = await connectProvider(service, "tiktok", "missing-refresh-code");
  assert.equal(
    incompleteTikTok.callback.headers.get("location"),
    "/?switch_social=error&provider=tiktok#switch-social",
  );
  instagramTokenMode = "malformed-long";
  const malformedLongInstagram = await connectProvider(service, "instagram", "malformed-long-code");
  assert.equal(
    malformedLongInstagram.callback.headers.get("location"),
    "/?switch_social=error&provider=instagram#switch-social",
  );
  instagramTokenMode = "zero-expiry";
  const zeroExpiryInstagram = await connectProvider(service, "instagram", "zero-expiry-code");
  assert.equal(
    zeroExpiryInstagram.callback.headers.get("location"),
    "/?switch_social=error&provider=instagram#switch-social",
  );
  assert.equal(instagramLongRequests, 2);
  assert.equal(profileRequests, 0);
  const finalAccounts = await service.api("/api/social/accounts");
  assert.deepEqual(finalAccounts.payload.accounts, []);
  const persisted = await readFile(path.join(dataDirectory, "social-store.json"), "utf8");
  assert.equal(persisted.includes("limited-access-secret"), false);
  assert.equal(persisted.includes("limited-refresh-secret"), false);
  assert.equal(persisted.includes("limited-instagram-secret"), false);
  assert.equal(persisted.includes("invalid-long-instagram-secret"), false);
});
