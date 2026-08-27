import assert from "node:assert/strict";
import { access, readFile } from "node:fs/promises";
import test from "node:test";

const rootFile = (path) => new URL(`../${path}`, import.meta.url);
const read = (path) => readFile(rootFile(path), "utf8");

test("le manifeste de la web app est installable et possede ses icones", async () => {
  const manifest = JSON.parse(await read("public/manifest.webmanifest"));

  assert.equal(manifest.display, "standalone");
  assert.equal(manifest.start_url, "/");
  assert.equal(manifest.scope, "/");
  assert.equal(manifest.theme_color, "#000000");
  assert.ok(manifest.icons.some((icon) => icon.sizes === "192x192"));
  assert.ok(manifest.icons.some((icon) => icon.sizes === "512x512"));

  await Promise.all([
    access(rootFile("public/apple-touch-icon.png")),
    ...manifest.icons.map((icon) => access(rootFile(`public${icon.src}`))),
  ]);
});

test("la page declare le manifeste et l'icone Apple", async () => {
  const html = await read("index.html");
  assert.match(html, /rel="manifest" href="\/manifest\.webmanifest"/);
  assert.match(html, /rel="apple-touch-icon"[^>]+href="\/apple-touch-icon\.png"/);
  assert.match(html, /apple-mobile-web-app-capable/);
  assert.match(html, /viewport-fit=cover/);
});

test("le service worker ne met jamais les API privees en cache", async () => {
  const worker = await read("public/service-worker.js");
  assert.match(worker, /new URL\(self\.location\.href\)\.searchParams\.get\("build"\)/);
  assert.match(worker, /key\.startsWith\(CACHE_PREFIX\) && key !== CACHE_NAME/);
  assert.match(worker, /url\.origin !== self\.location\.origin/);
  assert.match(worker, /url\.pathname\.startsWith\("\/api\/"\)/);
  assert.match(worker, /url\.pathname\.startsWith\("\/ws\/"\)/);
  assert.match(worker, /url\.pathname === "\/social"/);
  assert.match(worker, /url\.pathname\.startsWith\("\/social\/"\)/);
  assert.match(worker, /isSocialApplicationRequest\(url\)/);
  assert.match(worker, /url\.pathname === "\/reset-update\.html"/);
  assert.match(worker, /request\.mode === "navigate"/);
  assert.match(worker, /const networkFirstNavigation = async/);
  assert.match(worker, /await fetch\(request, \{ signal: controller\.signal \}\)/);
  assert.match(worker, /return \(await cache\.match\("\/"\)\) \?\?/);
  assert.doesNotMatch(worker, /if \(cached\)[\s\S]*?event\.waitUntil\(refresh/);
  assert.match(worker, /caches\.match\("\/offline\.html"\)/);
});

test("un cache PWA fige s'active sans renaviguer le client", async () => {
  const worker = await read("public/service-worker.js");
  // Une version de worker permet de forcer un nouveau script (compare octet par
  // octet, re-telecharge hors cache), meme si l'URL enregistree garde un vieux build.
  assert.match(worker, /const SW_VERSION = /);
  // A l'activation : purge l'index perime puis prend le controle sans lancer de
  // navigation. Attendre client.navigate() dans waitUntil() bloque Chromium.
  assert.match(worker, /await current\.delete\("\/"\)/);
  assert.match(worker, /await self\.clients\.claim\(\)/);
  assert.doesNotMatch(worker, /client\.navigate\(/);

  // Cote page : une verification du script est forcee a chaque chargement pour
  // ne pas attendre le throttle de 24 h du navigateur.
  const source = await read("src/pwa.ts");
  assert.match(source, /registration\.update\(\)/);
});

test("le chunk d'entree est importe dynamiquement pour reparer un index perime", async () => {
  const config = await read("vite.config.ts");
  // Le point d'entree n'est plus un <script src> statique : un index.html
  // perime (garde en cache par le service worker) qui reference un chunk deja
  // purge ne laisse plus le boot-splash tourner indefiniment. L'import() rejete
  // declenche un rechargement unique vers le build courant.
  assert.match(config, /cst-dynamic-entry-chunk/);
  assert.match(config, /enforce: "post"/);
  assert.match(config, /transformIndexHtml/);
  assert.match(config, /\.catch\(\(\) =>/);
  assert.match(config, /cst-chunk-build/);
  assert.match(config, /window\.location\.replace\(u\.toString\(\)\)/);
  // Le <script src="/src/main.ts"> statique reste la source : Vite le rewrite
  // en chunk hashe, puis ce plugin le convertit en import() dynamique.
  const indexHtml = await read("index.html");
  assert.match(indexHtml, /<script type="module" src="\/src\/main\.ts"><\/script>/);
});

test("l'aide Safari reconnait aussi le user-agent iPad de bureau", async () => {
  const source = await read("src/pwa.ts");
  assert.match(source, /Macintosh/);
  assert.match(source, /navigator\.maxTouchPoints > 1/);
  assert.match(source, /display-mode: standalone/);
  assert.match(source, /CstIOS \|\| nativeWindow\.CstAndroid/);
  assert.match(source, /window\.location\.protocol === "https:"/);
  assert.match(source, /service-worker\.js\?build=/);
  assert.match(source, /serviceWorker\.register\(SERVICE_WORKER_URL/);
});

test("le workflow iOS utilise un Mac distant et publie le build simulateur", async () => {
  const workflow = await read(".github/workflows/ios.yml");
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /runs-on: macos-15/);
  assert.match(workflow, /bash scripts\/build-ios\.sh simulator/);
  assert.match(workflow, /actions\/upload-artifact@v4/);
  assert.match(workflow, /CodexTerminal-iOS-Simulator/);
});
