import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize } from "node:path";
import { chromium } from "playwright-core";

const root = new URL("../", import.meta.url);
const dist = process.env.CST_PWA_DIST
  ? normalize(process.env.CST_PWA_DIST)
  : normalize(join(root.pathname.replace(/^\/(.:)/, "$1"), "dist"));
const privatePaths = ["/api/pwa-private-probe", "/ws/pwa-private-probe", "/mcp"];
const requestCounts = new Map();
const contentTypes = new Map([
  [".css", "text/css; charset=utf-8"],
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".png", "image/png"],
  [".webmanifest", "application/manifest+json"],
]);

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", "http://127.0.0.1");
  requestCounts.set(url.pathname, (requestCounts.get(url.pathname) ?? 0) + 1);

  if (privatePaths.includes(url.pathname)) {
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-type": "application/json; charset=utf-8",
    });
    response.end(JSON.stringify({ path: url.pathname, sequence: requestCounts.get(url.pathname) }));
    return;
  }

  const requestedPath = url.pathname === "/" ? "/index.html" : url.pathname;
  const candidate = normalize(join(dist, requestedPath));
  const isInsideDist = candidate.startsWith(`${dist}\\`) || candidate === dist;

  try {
    if (!isInsideDist || !(await stat(candidate)).isFile()) throw new Error("not found");
    const body = await readFile(candidate);
    response.writeHead(200, {
      "cache-control": url.pathname === "/service-worker.js" ? "no-cache" : "no-store",
      "content-type": contentTypes.get(extname(candidate)) ?? "application/octet-stream",
    });
    response.end(body);
  } catch {
    const body = await readFile(join(dist, "index.html"));
    response.writeHead(200, {
      "cache-control": "no-store",
      "content-type": "text/html; charset=utf-8",
    });
    response.end(body);
  }
});

const chromeCandidates = [
  process.env.CST_CHROME_PATH,
  "C:/Program Files/Google/Chrome/Application/chrome.exe",
  "C:/Program Files (x86)/Google/Chrome/Application/chrome.exe",
  `${process.env.LOCALAPPDATA || ""}/Google/Chrome/Application/chrome.exe`,
].filter(Boolean);
const executablePath = chromeCandidates.find(existsSync);
if (!executablePath) throw new Error("Chrome ou Chromium est introuvable.");

await new Promise((resolve, reject) => {
  server.once("error", reject);
  server.listen(0, "127.0.0.1", resolve);
});

const address = server.address();
assert(address && typeof address === "object");
const origin = `http://127.0.0.1:${address.port}`;
const browser = await chromium.launch({ executablePath, headless: true });
const context = await browser.newContext({ serviceWorkers: "allow" });
const page = await context.newPage();
let documentRequests = 0;
page.on("request", (request) => {
  if (request.isNavigationRequest() && request.frame() === page.mainFrame()) {
    documentRequests += 1;
  }
});

try {
  const workerStarted = context.waitForEvent("serviceworker");
  await page.goto(origin, { waitUntil: "domcontentloaded" });
  await workerStarted;
  await page.waitForTimeout(1_000);
  assert.equal(
    documentRequests,
    1,
    "l'activation du service worker ne doit pas relancer la navigation principale",
  );
  await page.evaluate(async () => {
    await navigator.serviceWorker.ready;
    if (!navigator.serviceWorker.controller) {
      await new Promise((resolve) => {
        navigator.serviceWorker.addEventListener("controllerchange", resolve, { once: true });
      });
    }
  });
  assert.equal(await page.evaluate(() => Boolean(navigator.serviceWorker.controller)), true);

  // Recharge une fois sous controle du worker afin de remplir le cache runtime des assets hashes.
  await page.reload({ waitUntil: "networkidle" });
  const cacheSnapshot = await page.evaluate(async () => {
    const names = await caches.keys();
    const entries = [];
    for (const name of names) {
      const cache = await caches.open(name);
      entries.push(...(await cache.keys()).map((request) => request.url));
    }
    return { names, entries };
  });
  assert.equal(cacheSnapshot.names.length, 1);
  assert.match(cacheSnapshot.names[0], /^codex-terminal-static-/);
  assert(cacheSnapshot.entries.some((url) => /\/assets\/index-[^/]+\.js$/.test(url)));
  assert(cacheSnapshot.entries.some((url) => url.endsWith("/offline.html")));

  for (const path of privatePaths) {
    const first = await page.evaluate(async (target) => (await fetch(target)).json(), path);
    const second = await page.evaluate(async (target) => (await fetch(target)).json(), path);
    assert.equal(first.sequence, 1, `${path} doit atteindre le serveur au premier appel`);
    assert.equal(second.sequence, 2, `${path} ne doit pas provenir d'un cache au second appel`);
  }
  const cachedAfterPrivateRequests = await page.evaluate(async () => {
    const entries = [];
    for (const name of await caches.keys()) {
      const cache = await caches.open(name);
      entries.push(...(await cache.keys()).map((request) => request.url));
    }
    return entries;
  });
  assert.equal(
    cachedAfterPrivateRequests.some((url) => privatePaths.some((path) => new URL(url).pathname === path)),
    false,
  );

  await context.setOffline(true);
  for (const path of privatePaths) {
    const failed = await page.evaluate(async (target) => {
      try {
        await fetch(target);
        return false;
      } catch {
        return true;
      }
    }, path);
    assert.equal(failed, true, `${path} doit echouer hors ligne sans reponse privee mise en cache`);
  }

  const offlineResponse = await page.goto(`${origin}/pwa-offline-probe`, {
    waitUntil: "domcontentloaded",
  });
  assert.equal(offlineResponse?.fromServiceWorker(), true);
  assert.equal(await page.title(), "Codex Terminal");
  assert.equal(await page.evaluate(() => Boolean(navigator.serviceWorker.controller)), true);

  process.stdout.write(
    `PWA runtime OK: 1 worker, ${cacheSnapshot.entries.length} ressources cachees, `
      + `${privatePaths.length} routes privees exclues, navigation hors ligne operationnelle.\n`,
  );
} finally {
  await context.setOffline(false).catch(() => undefined);
  await browser.close();
  await new Promise((resolve) => server.close(resolve));
}
