const CACHE_PREFIX = "codex-terminal-static-";
// Version du worker. Toute modification de cette valeur suffit a forcer le
// navigateur a installer un nouveau service worker : il re-telecharge ce script
// hors du cache HTTP (`updateViaCache: "none"`) et le compare octet par octet a
// celui installe. C'est le levier qui sort un onglet d'un cache PWA fige sans
// aucune manipulation de l'utilisateur, meme si l'URL enregistree pointe encore
// vers un ancien build.
const SW_VERSION = "5";
const BUILD_ID = new URL(self.location.href).searchParams.get("build") || "legacy";
const CACHE_NAME = `${CACHE_PREFIX}${BUILD_ID}`;
const NAVIGATION_NETWORK_TIMEOUT_MS = 5_000;
const PRECACHE_URLS = [
  "/",
  "/offline.html",
  "/manifest.webmanifest",
  "/apple-touch-icon.png",
  "/icons/pwa-192.png",
  "/icons/pwa-512.png",
];

self.addEventListener("install", (event) => {
  event.waitUntil(caches.open(CACHE_NAME).then((cache) => cache.addAll(PRECACHE_URLS)));
  self.skipWaiting();
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    (async () => {
      const keys = await caches.keys();
      await Promise.all(
        keys
          .filter((key) => key.startsWith(CACHE_PREFIX) && key !== CACHE_NAME)
          .map((key) => caches.delete(key)),
      );
      // Un index.html perime conserve dans le cache courant est la cause typique
      // d'un cache PWA fige : il reference d'anciens chunks /assets/ et se
      // ressert en boucle. On le purge pour que la prochaine navigation reparte
      // du reseau (les assets hashes restants sont simplement inutilises).
      const current = await caches.open(CACHE_NAME);
      await current.delete("/");
      await self.clients.claim();
      // Ne jamais naviguer les clients depuis `activate`: Chromium peut attendre
      // la fin de l'activation pour committer cette navigation, tandis que le
      // `waitUntil` attend lui-meme `client.navigate`, ce qui bloque la page.
      // Le document courant vient deja du nouveau build ; une navigation future
      // ou la recuperation des chunks Vite reprendra l'index frais si necessaire.
    })(),
  );
});

const isPrivateApplicationRequest = (url) =>
  url.pathname.startsWith("/api/")
  || url.pathname.startsWith("/ws/")
  || url.pathname === "/mcp";

const isSocialApplicationRequest = (url) =>
  url.pathname === "/social" || url.pathname.startsWith("/social/");

const cachedStaticAsset = async (request) => {
  const cache = await caches.open(CACHE_NAME);
  const cached = await cache.match(request);
  if (cached) return cached;

  try {
    const response = await fetch(request);
    if (response.ok) await cache.put(request, response.clone());
    return response;
  } catch (error) {
    // Le VPS est injoignable (Tailscale coupe, tunnel en cours de retablissement,
    // etc.). On ne doit pas laisser la promesse se rejeter en "Uncaught" pour
    // chaque asset absent du cache : on renvoie une reponse explicite qui evite
    // le bruit dans la console et permet au front de rester utilisable.
    return new Response("", {
      status: 504,
      statusText: "Gateway Timeout",
      headers: { "Cache-Control": "no-store" },
    });
  }
};

const networkFirstNavigation = async (request) => {
  const cache = await caches.open(CACHE_NAME);
  const controller = new AbortController();
  const timeout = setTimeout(() => controller.abort(), NAVIGATION_NETWORK_TIMEOUT_MS);

  try {
    const response = await fetch(request, { signal: controller.signal });
    if (response.ok) await cache.put("/", response.clone()).catch(() => undefined);
    return response;
  } catch {
    return (await cache.match("/")) ?? (await caches.match("/offline.html"));
  } finally {
    clearTimeout(timeout);
  }
};

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;

  const url = new URL(request.url);
  if (
    url.origin !== self.location.origin
    || isPrivateApplicationRequest(url)
    || isSocialApplicationRequest(url)
  ) return;
  // Cette page est la porte de sortie d'un cache PWA obsolète. Si le service
  // worker la remplace par la navigation mise en cache, elle ne peut jamais
  // désinscrire l'ancien worker ni purger les caches.
  if (url.pathname === "/reset-update.html") return;

  if (request.mode === "navigate") {
    event.respondWith(networkFirstNavigation(request));
    return;
  }

  if (
    url.pathname.startsWith("/assets/")
    || url.pathname.startsWith("/icons/")
    || url.pathname === "/apple-touch-icon.png"
    || url.pathname === "/manifest.webmanifest"
  ) {
    event.respondWith(cachedStaticAsset(request));
  }
});
