// Onglet « Réseaux sociaux » : embarque le dashboard Social Analytics de Switch
// développement (vues quotidiennes par compte Instagram/TikTok, détails par
// Reel et vidéo) dans une vraie vue de l'application, au lieu du dialogue
// injecté par l'ancien social-loader.js.
//
// Le dashboard vit dans le sidecar social (même origine, via social-gateway) ;
// cet onglet le charge dans un iframe same-origin. L'iframe est conservée dans
// le module et ré-adoptée par un MutationObserver : les re-rendus de l'app ne
// la détruisent pas, le dashboard ne se recharge donc pas à chaque rendu.
import "./social-view.css";

export type SocialCallbackStatus = "connected" | "error";

type SocialPanelOptions = {
  remoteMode: boolean;
};

type SocialPanelBindings = {
  rerender: () => void;
};

const FRAME_SANDBOX =
  "allow-scripts allow-same-origin allow-forms allow-popups allow-popups-to-escape-sandbox allow-top-navigation-by-user-activation";

let panelVisible = false;
let availability: "unknown" | "available" | "unavailable" = "unknown";
let probing = false;
let frame: HTMLIFrameElement | null = null;
let frameUrl = "/social/";
let appObserver: MutationObserver | null = null;
let lastRerender: (() => void) | null = null;

const escapeHtml = (value: string): string =>
  String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");

const createFrame = (): HTMLIFrameElement => {
  const iframe = document.createElement("iframe");
  iframe.className = "social-dashboard-frame";
  iframe.title = "Réseaux sociaux — vues Instagram et TikTok";
  iframe.src = frameUrl;
  iframe.setAttribute("sandbox", FRAME_SANDBOX);
  iframe.setAttribute("loading", "eager");
  frame = iframe;
  return iframe;
};

const adoptFrame = (): void => {
  if (availability !== "available" || !panelVisible) return;
  const host = document.querySelector<HTMLElement>(".social-dashboard-host");
  if (!host) return;
  if (!frame) createFrame();
  if (frame && frame.parentElement !== host) host.appendChild(frame);
};

const observeApp = (): void => {
  if (appObserver) return;
  const root = document.querySelector<HTMLElement>("#app");
  if (!root) return;
  appObserver = new MutationObserver(() => adoptFrame());
  appObserver.observe(root, { childList: true, subtree: true });
};

const probeAvailability = async (): Promise<void> => {
  if (probing) return;
  probing = true;
  let available = false;
  const controller = new AbortController();
  const timeout = window.setTimeout(() => controller.abort(), 2_500);
  try {
    const response = await fetch("/api/social/availability", {
      credentials: "same-origin",
      cache: "no-store",
      headers: { accept: "application/json" },
      signal: controller.signal,
    });
    const payload = response.ok ? await response.json() : null;
    available = payload?.available === true;
  } catch {
    available = false;
  } finally {
    window.clearTimeout(timeout);
    probing = false;
    availability = available ? "available" : "unavailable";
  }
  lastRerender?.();
};

/**
 * Retour d'OAuth : le fournisseur a redirigé sur `/?switch_social=...`.
 * On charge l'iframe avec le statut en paramètre pour que le dashboard
 * affiche son toast de confirmation, puis on revient à l'URL nue.
 */
export const setSocialCallbackResult = (
  status: SocialCallbackStatus,
  provider: string,
): void => {
  const encodedProvider = encodeURIComponent(provider || "account");
  frameUrl = status === "connected"
    ? `/social/?connected=${encodedProvider}`
    : `/social/?connect_error=${encodedProvider}`;
  if (frame) {
    frame.src = frameUrl;
    frameUrl = "/social/";
  }
};

const renderStatusCard = (
  icon: string,
  title: string,
  description: string,
  retry = false,
  link = true,
): string => `<section class="social-dashboard-panel">
  <div class="social-status-card" role="status">
    <span class="social-status-icon"><i data-lucide="${icon}"></i></span>
    <strong>${escapeHtml(title)}</strong>
    <p>${escapeHtml(description)}</p>
    <div class="social-status-actions">
      ${retry ? `<button type="button" class="tool-button" data-social-retry><i data-lucide="refresh-cw"></i><span>Réessayer</span></button>` : ""}
      ${link ? `<a class="tool-button primary" href="/social/" target="_blank" rel="noopener noreferrer"><i data-lucide="external-link"></i><span>Ouvrir le dashboard</span></a>` : ""}
    </div>
  </div>
</section>`;

export const renderSocialPanel = ({ remoteMode }: SocialPanelOptions): string => {
  if (!remoteMode) {
    return renderStatusCard(
      "server",
      "Connexion au serveur requise",
      "L'onglet Réseaux sociaux lit les vues Instagram et TikTok depuis le dashboard Social Analytics du serveur Switch. Reconnectez l'application à un serveur pour l'utiliser.",
      false,
      false,
    );
  }
  if (availability === "unavailable") {
    return renderStatusCard(
      "wifi-off",
      "Dashboard social indisponible",
      "Le service Social Analytics n'est pas joignable sur ce serveur (sidecar non déployé ou redémarrage en cours). Réessayez, ou ouvrez le dashboard dans une page dédiée.",
      true,
    );
  }
  if (availability === "unknown") {
    return `<section class="social-dashboard-panel">
      <div class="social-status-card" role="status">
        <span class="social-status-icon is-spinning"><i data-lucide="loader-circle"></i></span>
        <strong>Connexion au dashboard social…</strong>
        <p>Vérification de la disponibilité du service Social Analytics.</p>
      </div>
    </section>`;
  }
  return `<section class="social-dashboard-panel">
    <div class="social-dashboard-host"></div>
  </section>`;
};

export const bindSocialPanel = ({ rerender }: SocialPanelBindings): void => {
  document.querySelector<HTMLButtonElement>("[data-social-retry]")?.addEventListener("click", () => {
    availability = "unknown";
    void probeAvailability().then(rerender);
  });
};

export const activateSocialPanel = (
  rerender: () => void,
  remoteMode: boolean,
): void => {
  panelVisible = true;
  lastRerender = rerender;
  if (!remoteMode) {
    availability = "unavailable";
    rerender();
    return;
  }
  observeApp();
  if (availability === "unknown") {
    void probeAvailability();
  } else {
    adoptFrame();
  }
};

export const deactivateSocialPanel = (): void => {
  panelVisible = false;
  lastRerender = null;
};
