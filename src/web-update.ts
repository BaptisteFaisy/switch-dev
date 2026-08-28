const WEB_UPDATE_POLL_INTERVAL_MS = 5_000;

export const frontendBuildIdentity = (html: string): string | null => {
  const marker = html.match(
    /<meta\b[^>]*\bname\s*=\s*["']cst-build-id["'][^>]*>/i,
  )?.[0];
  const identity = marker?.match(/\bcontent\s*=\s*["']([^"']+)["']/i)?.[1]?.trim() ?? "";
  return identity || null;
};

let observedBuild: string | null = __CST_BUILD_ID__.trim() || null;
let poll: number | null = null;
let checkInFlight = false;
let reloading = false;

const refreshToLatestBuild = async () => {
  if (reloading) return;
  reloading = true;
  try {
    const registration = await navigator.serviceWorker?.getRegistration();
    await registration?.update();
    // Un index.html perime garde en cache par le service worker est la cause
    // classique d'une boucle de rechargement : la sonde detecte le nouveau
    // build, le reload retombe sur l'ancien HTML en cache (reseau coupe,
    // fallback network-first), et ainsi de suite. Purger l'entree de
    // navigation de tous les caches garantit que le reload reparte du HTML
    // frais servi par le serveur, ou de /offline.html sans sonde.
    if ("caches" in window) {
      const cacheKeys = await caches.keys();
      await Promise.all(
        cacheKeys.map(async (key) => {
          const cache = await caches.open(key);
          await Promise.all([
            cache.delete("/"),
            cache.delete(new Request(window.location.origin + "/")),
          ]);
        }),
      );
    }
  } catch {
    // Le HTML est servi en no-cache : le reload suffit meme sans service worker.
  }
  window.location.reload();
};

export const checkForWebUpdate = async (): Promise<void> => {
  if (checkInFlight || reloading) return;
  checkInFlight = true;
  try {
    // Comparer deux frontends. Le backend peut rester volontairement sur son
    // commit precedent lors d'une publication CSS/JS sans redemarrage ; le
    // comparer au bundle courant provoquait alors un reload toutes les 5 s.
    const response = await fetch("/", {
      cache: "no-store",
      headers: { Accept: "text/html" },
    });
    if (!response.ok) return;
    const identity = frontendBuildIdentity(await response.text());
    if (!identity) return;
    if (observedBuild === null) {
      observedBuild = identity;
      return;
    }
    if (identity !== observedBuild) await refreshToLatestBuild();
  } catch {
    // Une courte coupure est normale pendant la bascule atomique du serveur.
  } finally {
    checkInFlight = false;
  }
};

export const initWebAutoUpdate = () => {
  if (!(["http:", "https:"] as string[]).includes(window.location.protocol)) return;
  void checkForWebUpdate();
  if (poll === null) {
    poll = window.setInterval(() => void checkForWebUpdate(), WEB_UPDATE_POLL_INTERVAL_MS);
  }
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") void checkForWebUpdate();
  });
};
