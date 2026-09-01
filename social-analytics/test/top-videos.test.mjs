import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { mkdir, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

const serviceDirectory = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const proxyKey = "social-test-proxy-key-32-bytes-minimum";

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

async function launchAuthProvider() {
  const server = createServer((request, response) => {
    const url = new URL(request.url, "http://127.0.0.1");
    if (url.pathname === "/api/auth/me") {
      const cookie = request.headers.cookie ?? "";
      const session = /cst_session=([^;]+)/.exec(cookie)?.[1];
      if (session) sendJson(response, 200, { user: { id: session, username: session } });
      else sendJson(response, 401, { ok: false });
      return;
    }
    sendJson(response, 404, { ok: false });
  });
  const port = await listen(server);
  return { server, port };
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

async function launchService(dataDirectory, bridgeDirectory, authPort) {
  const servicePort = await reservePort();
  let stderr = "";
  const child = spawn(process.execPath, [path.join(serviceDirectory, "server.mjs")], {
    cwd: serviceDirectory,
    env: {
      ...process.env,
      PORT: String(servicePort),
      SOCIAL_DATA_DIR: dataDirectory,
      SOCIAL_BRIDGE_DIR: bridgeDirectory,
      SOCIAL_TIMEZONE: "Europe/Paris",
      SOCIAL_DEMO_MODE: "false",
      SOCIAL_DEMO_SEED: "false",
      SOCIAL_SCHEDULED_SYNC_ENABLED: "false",
      SOCIAL_PROXY_KEY: proxyKey,
      SOCIAL_PUBLIC_ORIGIN: `http://127.0.0.1:${servicePort}`,
      SWITCH_AUTH_URL: `http://127.0.0.1:${authPort}/api/auth/me`,
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk) => { stderr += chunk; });
  const baseUrl = `http://127.0.0.1:${servicePort}`;
  await waitUntilReady(baseUrl, child, () => stderr);

  async function api(pathname) {
    const response = await fetch(`${baseUrl}${pathname}`, {
      headers: {
        "x-social-proxy-key": proxyKey,
        cookie: "cst_session=user-a",
      },
    });
    return { response, payload: await response.json() };
  }

  return { baseUrl, child, api, diagnostics: () => stderr };
}

function stopChild(child) {
  return new Promise((resolve) => {
    const timeout = setTimeout(resolve, 1_500);
    child.once("exit", () => {
      clearTimeout(timeout);
      resolve();
    });
    child.kill();
  });
}

function seedStore(dataDirectory) {
  return writeFile(path.join(dataDirectory, "social-store.json"), JSON.stringify({
    version: 2,
    accounts: [
      {
        id: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        ownerUserId: "user-a",
        platform: "instagram",
        handle: "atelier.noa",
        displayName: "Atelier Noa",
        colour: "#f37fd5",
        connectionMode: "live",
        status: "connected",
        externalAccountId: "ig-canonical",
        appScopedAccountId: "ig-app-scoped",
        createdAt: "2026-08-01T00:00:00.000Z",
        syncedAt: "2026-08-30T00:00:00.000Z",
      },
    ],
    metrics: [],
    media: [
      {
        id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1",
        ownerUserId: "user-a",
        accountId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        platform: "instagram",
        externalMediaId: "ig-reel-1",
        title: "Reel institutionnel",
        permalink: "https://www.instagram.com/reel/ig-reel-1/",
        thumbnailUrl: "https://example.test/thumb1.jpg",
        publishedAt: "2026-08-10T00:00:00.000Z",
      },
      {
        id: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2",
        ownerUserId: "user-a",
        accountId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        platform: "instagram",
        externalMediaId: "ig-reel-2",
        title: "Reel discret",
        permalink: "https://www.instagram.com/reel/ig-reel-2/",
        thumbnailUrl: null,
        publishedAt: "2026-08-11T00:00:00.000Z",
      },
    ],
    mediaSnapshots: [
      {
        id: "cccccccc-cccc-4ccc-8ccc-ccccccccccc1",
        ownerUserId: "user-a",
        accountId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        mediaId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1",
        date: "2026-08-29",
        views: 5000,
        capturedAt: "2026-08-29T22:00:00.000Z",
      },
      {
        id: "cccccccc-cccc-4ccc-8ccc-ccccccccccc2",
        ownerUserId: "user-a",
        accountId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        mediaId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb2",
        date: "2026-08-29",
        views: 4000,
        capturedAt: "2026-08-29T22:00:00.000Z",
      },
      {
        id: "cccccccc-cccc-4ccc-8ccc-ccccccccccc3",
        ownerUserId: "user-b",
        accountId: "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        mediaId: "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbb1",
        date: "2026-08-29",
        views: 999,
        capturedAt: "2026-08-29T22:00:00.000Z",
      },
    ],
    oauthAttempts: [],
    demoSeededOwners: [],
  }, null, 2), "utf8");
}

function bridgeEvent(source, handle, platform, generatedAt, videos, idempotencyDate) {
  return {
    schema: "switch.social.media_views/1",
    generatedAt,
    account: { handle, ...(platform ? { platform } : {}) },
    summary: {
      videoCount: videos.length,
      totalViews: videos.reduce((sum, video) => sum + (video.views ?? 0), 0),
      mostViewed: videos.reduce((best, video) => !best || (video.views ?? 0) > (best.views ?? 0) ? video : best, null),
    },
    videos,
    idempotencyKey: `${handle}@${idempotencyDate}`,
    source,
  };
}

test("top-videos : fusion officiel + pont YouTube/TikTok, tri par vues et déduplication", { timeout: 60_000 }, async (t) => {
  const dataDirectory = await mkdtemp(path.join(tmpdir(), "switch-social-top-\u0074est-"));
  const bridgeDirectory = path.join(dataDirectory, "bridge");
  await mkdir(bridgeDirectory, { recursive: true });
  await seedStore(dataDirectory);
  // Événement YouTube ancien puis plus récent pour la même chaîne : le plus
  // récent doit primer (y compris pour une vidéo présente dans les deux).
  await writeFile(path.join(bridgeDirectory, "media-views-noastudio-20260830T2201.json"), JSON.stringify(
    bridgeEvent("youtube-scraper", "noastudio", "youtube", "2026-08-30T22:01:00.000Z", [
      { id: "aaa", title: "Vidéo phare", views: 1000000, thumbnail: "https://i.ytimg.com/vi/aaa/hqdefault.jpg", url: "https://www.youtube.com/watch?v=aaa", uploadDate: "20260830" },
      { id: "bbb", title: "Deuxième", views: 200000, thumbnail: null, url: "https://www.youtube.com/watch?v=bbb", uploadDate: "20260830" },
    ], "noastudio@20260830T2201"),
  ), "utf8");
  // Un re-scrap produit un inventaire complet : les vidéos restées en ligne
  // sont présentes dans l'événement le plus récent avec leur compteur à jour.
  await writeFile(path.join(bridgeDirectory, "media-views-noastudio-20260830T2215.json"), JSON.stringify(
    bridgeEvent("youtube-scraper", "noastudio", "youtube", "2026-08-30T22:15:00.000Z", [
      { id: "aaa", title: "Vidéo phare (màj)", views: 1500000, thumbnail: "https://i.ytimg.com/vi/aaa/hqdefault.jpg", url: "https://www.youtube.com/watch?v=aaa", uploadDate: "20260830" },
      { id: "bbb", title: "Deuxième", views: 200000, thumbnail: null, url: "https://www.youtube.com/watch?v=bbb", uploadDate: "20260830" },
      { id: "ccc", title: "Nouveauté", views: 50000, thumbnail: null, url: "https://www.youtube.com/watch?v=ccc", uploadDate: "20260831" },
    ], "noastudio@20260830T2215"),
  ), "utf8");
  await writeFile(path.join(bridgeDirectory, "media-views-duelloapp-20260830T2201.json"), JSON.stringify(
    bridgeEvent("tiktok-views-api", "duello_app", null, "2026-08-30T22:01:00.000Z", [
      { id: "tt1", desc: "TikTok viral", views: 300000, url: "https://www.tiktok.com/@duello_app/video/tt1", uploadDate: "20260830" },
    ], "duelloapp@20260830T2201"),
  ), "utf8");
  await writeFile(path.join(bridgeDirectory, "autre-fichier.json"), JSON.stringify({ schema: "autre" }), "utf8");

  const auth = await launchAuthProvider();
  const service = await launchService(dataDirectory, bridgeDirectory, auth.port);
  t.after(async () => {
    await stopChild(service.child);
    await new Promise((resolve) => auth.server.close(resolve));
    await rm(dataDirectory, { recursive: true, force: true });
  });

  const { response, payload } = await service.api("/api/social/top-videos?limit=20");
  assert.equal(response.status, 200);
  assert.equal(payload.ok, true);
  assert.deepEqual(payload.items.map((item) => item.views), [1500000, 300000, 200000, 50000, 5000, 4000]);
  const byExternalId = new Map(payload.items.map((item) => [item.externalId, item]));

  // La vidéo présente dans deux événements n'apparaît qu'une fois, avec le
  // compteur du plus récent.
  assert.equal(byExternalId.size, payload.items.length);
  const phare = byExternalId.get("aaa");
  assert.equal(phare.views, 1500000);
  assert.equal(phare.platform, "youtube");
  assert.equal(phare.accountHandle, "noastudio");
  assert.equal(phare.title, "Vidéo phare (màj)");
  assert.equal(phare.thumbnailUrl, "https://i.ytimg.com/vi/aaa/hqdefault.jpg");
  assert.equal(phare.permalink, "https://www.youtube.com/watch?v=aaa");
  assert.equal(phare.publishedAt, "2026-08-30T12:00:00.000Z");
  assert.equal(phare.source, "youtube-scraper");

  const reel = byExternalId.get("ig-reel-1");
  assert.equal(reel.views, 5000);
  assert.equal(reel.platform, "instagram");
  assert.equal(reel.accountHandle, "atelier.noa");
  assert.equal(reel.source, "official");
  assert.equal(reel.thumbnailUrl, "https://example.test/thumb1.jpg");

  const tik = byExternalId.get("tt1");
  assert.equal(tik.platform, "tiktok");
  assert.equal(tik.accountHandle, "duello_app");
  assert.equal(tik.permalink, "https://www.tiktok.com/@duello_app/video/tt1");
  assert.deepEqual([...payload.platforms].sort(), ["instagram", "tiktok", "youtube"]);

  // Filtre par plateforme.
  const youtube = await service.api("/api/social/top-videos?platforms=youtube&limit=20");
  assert.equal(youtube.response.status, 200);
  assert.ok(youtube.payload.items.every((item) => item.platform === "youtube"));
  assert.equal(youtube.payload.items.length, 3);

  // Limite.
  const limited = await service.api("/api/social/top-videos?limit=3");
  assert.equal(limited.response.status, 200);
  assert.equal(limited.payload.items.length, 3);

  // Filtre par compte : restreint l'officiel, le pont reste global.
  const accounts = await service.api("/api/social/top-videos?accounts=aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa&limit=20");
  assert.equal(accounts.response.status, 200);
  const handles = accounts.payload.items.map((item) => item.accountHandle);
  assert.ok(handles.includes("atelier.noa"));
  assert.ok(handles.includes("duello_app"));

  // Sans événement du pont : le service reste sain.
  const emptyBridgeService = await launchService(dataDirectory, path.join(dataDirectory, "vide"), auth.port);
  t.after(() => stopChild(emptyBridgeService.child));
  const empty = await emptyBridgeService.api("/api/social/top-videos?limit=20");
  assert.equal(empty.response.status, 200);
  assert.ok(empty.payload.items.every((item) => item.source === "official"));
});