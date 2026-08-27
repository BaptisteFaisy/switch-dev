const WEB_UPDATE_POLL_INTERVAL_MS = 5_000;

type HealthBuild = {
  version?: string;
  commit?: string;
};

const GIT_COMMIT_PATTERN = /^[0-9a-f]{7,40}$/i;

const normalizedGitCommit = (value: string | undefined): string | null => {
  const commit = value?.trim() ?? "";
  return GIT_COMMIT_PATTERN.test(commit) ? commit.toLowerCase() : null;
};

const sameBuildIdentity = (left: string, right: string): boolean => {
  if (left === right) return true;
  if (!GIT_COMMIT_PATTERN.test(left) || !GIT_COMMIT_PATTERN.test(right)) return false;
  // Le frontend et le serveur peuvent embarquer le meme SHA avec des longueurs
  // differentes (par exemple 7 et 40 caracteres).
  return left.startsWith(right) || right.startsWith(left);
};

let observedBuild: string | null = normalizedGitCommit(__CST_BUILD_COMMIT__);
let poll: number | null = null;
let checkInFlight = false;
let reloading = false;

const buildIdentity = (health: HealthBuild): string | null => {
  const commit = normalizedGitCommit(health.commit);
  if (commit) return commit;
  // Un deploiement frontend seul peut laisser le serveur sur un identifiant de
  // release (ex. "switch-vps-...") qui n'est pas comparable au SHA du bundle.
  // Dans ce cas, ignorer la sonde evite un rechargement permanent. Le fallback
  // par version ne sert que si le frontend n'a lui-meme aucun SHA exploitable.
  if (observedBuild !== null) return null;
  const version = health.version?.trim() ?? "";
  return version || null;
};

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
    const response = await fetch("/healthz", {
      cache: "no-store",
      headers: { Accept: "application/json" },
    });
    if (!response.ok) return;
    const identity = buildIdentity(await response.json() as HealthBuild);
    if (!identity) return;
    if (observedBuild === null) {
      observedBuild = identity;
      return;
    }
    if (!sameBuildIdentity(identity, observedBuild)) await refreshToLatestBuild();
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
