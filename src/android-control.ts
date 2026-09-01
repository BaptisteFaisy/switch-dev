import { invoke } from "./platform";
import "./android-control.css";

export type AndroidControlPanelOptions = {
  remoteMode: boolean;
};

export type AndroidControlUiOptions = {
  rerender: () => void;
  setStatus?: (message: string) => void;
};

type AndroidDevice = {
  serial: string;
  state: "device" | "unauthorized" | "offline" | "unknown";
  transport: "usb" | "emulator" | "network" | "unknown";
  model?: string | null;
  product?: string | null;
  tikmatrixManaged: boolean;
};

type AndroidDevicesSnapshot = {
  devices: string[];
  deviceDetails: AndroidDevice[];
  bridgeOnline: boolean;
  connectorOnline: boolean;
  scrcpyAvailable: boolean;
  wsScrcpyAvailable: boolean;
  wsScrcpyOnline: boolean;
  adbError: string | null;
};

type ScrcpySetupAction = {
  id: string;
  action: "open_scrcpy" | "start_ws_scrcpy";
  status: "queued" | "claimed" | "submitted" | "failed";
  deviceSerial: string;
  detail?: string | null;
};

type AndroidControlFeedback = {
  tone: "success" | "warning" | "error";
  message: string;
};

type AndroidEmbedState = "closed" | "loading" | "ready" | "error";
type AndroidEmbedSession = {
  state: Exclude<AndroidEmbedState, "closed">;
  error: string;
};
type WsScrcpyServiceState = "idle" | "starting" | "online" | "error";

const POLL_INTERVAL_MS = 4_000;
const MAX_ANDROID_LIVE_SCREENS = 40;
const SCRCPY_EMBED_ORIGIN = "http://127.0.0.1:8000";
const SCRCPY_EMBED_PATH = "/embed.html";
const SCRCPY_EMBED_URL_PREFIX = "http://127.0.0.1:8000/embed.html?device=";
const SCRCPY_EMBED_LOAD_TIMEOUT_MS = 12_000;
// Couvre aussi la première installation automatique (téléchargement d'environ
// 80 Mo + extraction) déclenchée par le connecteur Windows.
const WS_SCRCPY_START_TIMEOUT_MS = 150_000;

let snapshot: AndroidDevicesSnapshot | null = null;
let snapshotSignature = "";
let loading = false;
let error = "";
let feedback: AndroidControlFeedback | null = null;
let openingExternalScrcpy = false;
let wsScrcpyServiceState: WsScrcpyServiceState = "idle";
let wsScrcpyServiceError = "";
let wsScrcpyStartTimer: number | null = null;
let selectedDevice = "";
let visible = false;
let pollTimer: number | null = null;
let panelRerender: (() => void) | null = null;
let panelSetStatus: ((message: string) => void) | null = null;
const androidEmbedSessions = new Map<string, AndroidEmbedSession>();
const androidEmbedLoadTimers = new Map<string, number>();
let androidEmbedMessageBound = false;

const escapeHtml = (value: string): string =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");

const escapeAttr = escapeHtml;

const readableError = (value: unknown): string => {
  const text = String(value instanceof Error ? value.message : value).trim();
  return text || "Une erreur inconnue est survenue.";
};

const buildScrcpyEmbedUrl = (deviceSerial: string): string | null => {
  const serial = deviceSerial.trim();
  if (!serial || serial !== deviceSerial || /[\u0000-\u001f\u007f]/.test(serial)) return null;

  const url = new URL(SCRCPY_EMBED_PATH, `${SCRCPY_EMBED_ORIGIN}/`);
  url.searchParams.set("device", serial);
  const queryKeys = [...url.searchParams.keys()];
  if (
    url.origin !== SCRCPY_EMBED_ORIGIN
    || url.protocol !== "http:"
    || url.hostname !== "127.0.0.1"
    || url.port !== "8000"
    || url.pathname !== SCRCPY_EMBED_PATH
    || url.username !== ""
    || url.password !== ""
    || url.hash !== ""
    || queryKeys.length !== 1
    || queryKeys[0] !== "device"
    || url.searchParams.get("device") !== serial
    || !url.href.startsWith(SCRCPY_EMBED_URL_PREFIX)
  ) {
    return null;
  }
  return url.href;
};

const clearAndroidEmbedLoadTimer = (deviceSerial?: string): void => {
  if (deviceSerial) {
    const timer = androidEmbedLoadTimers.get(deviceSerial);
    if (timer !== undefined) window.clearTimeout(timer);
    androidEmbedLoadTimers.delete(deviceSerial);
    return;
  }
  for (const timer of androidEmbedLoadTimers.values()) window.clearTimeout(timer);
  androidEmbedLoadTimers.clear();
};

const clearWsScrcpyStartTimer = (): void => {
  if (wsScrcpyStartTimer === null) return;
  window.clearTimeout(wsScrcpyStartTimer);
  wsScrcpyStartTimer = null;
};

const stopPolling = (): void => {
  if (pollTimer === null) return;
  window.clearInterval(pollTimer);
  pollTimer = null;
};

const resetAndroidEmbed = (deviceSerial?: string): void => {
  if (deviceSerial) {
    clearAndroidEmbedLoadTimer(deviceSerial);
    androidEmbedSessions.delete(deviceSerial);
    return;
  }
  clearAndroidEmbedLoadTimer();
  androidEmbedSessions.clear();
};

const overallAndroidEmbedState = (): AndroidEmbedState => {
  const sessions = [...androidEmbedSessions.values()];
  if (!sessions.length) return "closed";
  if (sessions.some((session) => session.state === "error")) return "error";
  if (sessions.some((session) => session.state === "loading")) return "loading";
  return "ready";
};

const androidEmbedSummary = (): string => {
  const sessions = [...androidEmbedSessions.values()];
  const ready = sessions.filter((session) => session.state === "ready").length;
  const loading = sessions.filter((session) => session.state === "loading").length;
  const failed = sessions.filter((session) => session.state === "error").length;
  if (!sessions.length) return `0/${MAX_ANDROID_LIVE_SCREENS} écran actif`;
  const details = [
    `${ready} en direct`,
    loading ? `${loading} en connexion` : "",
    failed ? `${failed} en erreur` : "",
  ].filter(Boolean).join(" · ");
  return `${sessions.length}/${MAX_ANDROID_LIVE_SCREENS} écrans · ${details}`;
};

const updateAndroidEmbedSummaryDom = (): void => {
  const panel = document.querySelector<HTMLElement>("[data-android-control-embed]");
  if (panel) panel.dataset.state = overallAndroidEmbedState();
  const summary = document.querySelector<HTMLElement>("[data-android-embed-summary]");
  if (summary) summary.textContent = androidEmbedSummary();
};

const setAndroidEmbedState = (
  deviceSerial: string,
  state: Exclude<AndroidEmbedState, "closed">,
  nextError = "",
): void => {
  const session = androidEmbedSessions.get(deviceSerial);
  if (!session) return;
  if (state !== "loading") clearAndroidEmbedLoadTimer(deviceSerial);
  session.state = state;
  session.error = nextError;

  const screen = [...document.querySelectorAll<HTMLElement>("[data-android-screen]")]
    .find((candidate) => candidate.dataset.device === deviceSerial);
  if (!screen) return;
  screen.dataset.state = state;
  screen.setAttribute("aria-busy", state === "loading" ? "true" : "false");
  const errorCopy = screen.querySelector<HTMLElement>("[data-android-control-embed-error]");
  if (errorCopy) errorCopy.textContent = nextError;
  updateAndroidEmbedSummaryDom();
};

const androidEmbedTheme = (): "dark" | "light" =>
  document.documentElement.dataset.theme === "light" ? "light" : "dark";

const handleAndroidEmbedMessage = (event: MessageEvent): void => {
  if (event.origin !== SCRCPY_EMBED_ORIGIN) return;
  const frame = [...document.querySelectorAll<HTMLIFrameElement>("[data-android-embed-frame]")]
    .find((candidate) => event.source === candidate.contentWindow);
  const deviceSerial = frame?.dataset.device ?? "";
  if (
    !frame
    || !deviceSerial
    || !androidEmbedSessions.has(deviceSerial)
    || !event.data
    || typeof event.data !== "object"
    || (event.data as { type?: unknown }).type !== "ws-scrcpy-web:theme-ready"
  ) {
    return;
  }

  frame.contentWindow?.postMessage(
    { type: "ws-scrcpy-web:theme", theme: androidEmbedTheme() },
    SCRCPY_EMBED_ORIGIN,
  );
  setAndroidEmbedState(deviceSerial, "ready");
};

const bindAndroidEmbedMessages = (): void => {
  if (androidEmbedMessageBound) return;
  window.addEventListener("message", handleAndroidEmbedMessage);
  androidEmbedMessageBound = true;
};

const unbindAndroidEmbedMessages = (): void => {
  if (!androidEmbedMessageBound) return;
  window.removeEventListener("message", handleAndroidEmbedMessage);
  androidEmbedMessageBound = false;
};

const deviceDetails = (): AndroidDevice[] => {
  if (snapshot?.deviceDetails?.length) return snapshot.deviceDetails;
  return (snapshot?.devices ?? []).map((serial) => ({
    serial,
    state: "device",
    transport: "unknown",
    tikmatrixManaged: true,
  }));
};

const readyDeviceSerials = (): string[] =>
  deviceDetails()
    .filter((device) => device.state === "device")
    .map((device) => device.serial);

const effectiveDevice = (): string => {
  const devices = readyDeviceSerials();
  if (selectedDevice && devices.includes(selectedDevice)) return selectedDevice;
  return devices.length === 1 ? devices[0] : "";
};

const syncSelectedDevice = (): void => {
  const devices = readyDeviceSerials();
  if (selectedDevice && devices.includes(selectedDevice)) return;
  selectedDevice = devices.length === 1 ? devices[0] : "";
};

const reconcileAndroidEmbedDevice = (): void => {
  const ready = new Set(readyDeviceSerials());
  for (const deviceSerial of androidEmbedSessions.keys()) {
    if (ready.has(deviceSerial)) continue;
    setAndroidEmbedState(
      deviceSerial,
      "error",
      `L’appareil ${deviceSerial} n’est plus disponible. Fermez cet écran ou reconnectez le téléphone.`,
    );
  }
};

const reconcileWsScrcpyService = (next: AndroidDevicesSnapshot): void => {
  const wasOnline = wsScrcpyServiceState === "online";
  if (next.bridgeOnline && next.wsScrcpyOnline) {
    const wasStarting = wsScrcpyServiceState === "starting";
    clearWsScrcpyStartTimer();
    wsScrcpyServiceState = "online";
    wsScrcpyServiceError = "";
    if (wasStarting) {
      feedback = {
        tone: "success",
        message: "ws-scrcpy-web est en ligne. Vous pouvez maintenant afficher l’appareil dans Switch.",
      };
      panelSetStatus?.("ws-scrcpy-web est en ligne");
    }
    return;
  }

  if (wsScrcpyServiceState === "starting" && !next.bridgeOnline) {
    clearWsScrcpyStartTimer();
    wsScrcpyServiceState = "error";
    wsScrcpyServiceError =
      "Le pont Windows s’est déconnecté pendant le démarrage de ws-scrcpy-web.";
    return;
  }

  if (wasOnline) {
    wsScrcpyServiceState = "idle";
    wsScrcpyServiceError = "";
    feedback = {
      tone: "warning",
      message: "ws-scrcpy-web ne répond plus sur le PC Windows.",
    };
    panelSetStatus?.("ws-scrcpy-web est hors ligne");
    for (const deviceSerial of androidEmbedSessions.keys()) {
      setAndroidEmbedState(
        deviceSerial,
        "error",
        "Le service local ws-scrcpy-web ne répond plus. Redémarrez-le puis rechargez cet écran.",
      );
    }
  }
};

const refreshAndroidDevices = async (
  rerender: () => void,
  options: { silent?: boolean } = {},
): Promise<boolean> => {
  if (loading) return false;
  loading = true;
  if (!options.silent) {
    error = "";
    rerender();
  }

  try {
    const next = await invoke<AndroidDevicesSnapshot>("list_tiktok_sender_accounts");
    const nextSignature = JSON.stringify(next);
    const changed = nextSignature !== snapshotSignature;
    snapshot = next;
    snapshotSignature = nextSignature;
    error = "";
    reconcileWsScrcpyService(next);
    syncSelectedDevice();
    reconcileAndroidEmbedDevice();
    loading = false;
    if (changed || !options.silent) rerender();
    return true;
  } catch (cause) {
    const nextError = readableError(cause);
    const changed = nextError !== error || snapshot !== null;
    error = nextError;
    snapshot = null;
    snapshotSignature = "";
    loading = false;
    if (changed || !options.silent) rerender();
    return false;
  }
};

const transportLabel = (transport: AndroidDevice["transport"]): string => ({
  usb: "Téléphone USB",
  emulator: "Émulateur",
  network: "ADB réseau",
  unknown: "Appareil Android",
})[transport];

const stateLabel = (state: AndroidDevice["state"]): string => ({
  device: "Prêt",
  unauthorized: "Autorisation USB requise",
  offline: "Hors ligne",
  unknown: "État inconnu",
})[state];

const deviceLabel = (device: AndroidDevice): string =>
  device.model?.trim() || device.product?.trim() || device.serial;

const renderConnectionStatus = (): string => {
  if (!snapshot) {
    return `<span class="android-control-connection is-offline"><i data-lucide="wifi-off"></i>ADB non vérifié</span>`;
  }
  if (snapshot.bridgeOnline || snapshot.connectorOnline) {
    return `<span class="android-control-connection is-online"><i data-lucide="usb"></i>Pont Android en ligne</span>`;
  }
  return `<span class="android-control-connection is-offline"><i data-lucide="wifi-off"></i>Pont Android hors ligne</span>`;
};

const renderDevicePicker = (): string => {
  const devices = deviceDetails();
  if (!devices.length) {
    return `<div class="android-control-device-empty">
      <i data-lucide="usb"></i>
      <span>
        <strong>Aucun appareil ADB détecté</strong>
        <small>Branchez le téléphone au PC Windows et activez le débogage USB.</small>
      </span>
    </div>`;
  }

  return `<label class="android-control-device-picker" for="androidControlDevice">
    <span>Appareil ADB</span>
    <select id="androidControlDevice">
      <option value="">Choisir un appareil…</option>
      ${devices.map((device) => {
        const ready = device.state === "device";
        const selected = effectiveDevice() === device.serial;
        return `<option value="${escapeAttr(device.serial)}" ${selected ? "selected" : ""} ${ready ? "" : "disabled"}>${escapeHtml(deviceLabel(device))} · ${escapeHtml(transportLabel(device.transport))} · ${escapeHtml(stateLabel(device.state))}</option>`;
      }).join("")}
    </select>
    <small>Les téléphones USB doivent accepter la demande de débogage affichée sur leur écran.</small>
  </label>`;
};

const effectiveWsScrcpyServiceState = (): WsScrcpyServiceState =>
  snapshot?.bridgeOnline === true && snapshot.wsScrcpyOnline === true
    ? "online"
    : wsScrcpyServiceState;

const renderWsScrcpyServiceButton = (): string => {
  const state = effectiveWsScrcpyServiceState();
  const bridgeReady = snapshot?.bridgeOnline === true || snapshot?.connectorOnline === true;
  const launcherAvailable = snapshot?.wsScrcpyAvailable === true;
  const retry = state === "error";
  const disabled = state === "online" || state === "starting" || !bridgeReady || !launcherAvailable;
  const icon = state === "starting"
    ? "loader-circle"
    : state === "online"
      ? "badge-check"
      : retry
        ? "refresh-cw"
        : "play";
  const label = state === "starting"
    ? "Démarrage…"
    : state === "online"
      ? "Service en ligne"
      : retry
        ? "Réessayer le démarrage"
        : "Démarrer ws-scrcpy-web";

  return `<button
    type="button"
    id="androidControlStartWebScrcpy"
    class="tool-button android-control-service-button is-${state}"
    ${disabled ? "disabled" : ""}
    aria-busy="${state === "starting" ? "true" : "false"}"
  >
    <i data-lucide="${icon}" class="${state === "starting" ? "is-spinning" : ""}"></i>
    <span>${label}</span>
  </button>`;
};

const androidEmbedStateLabel = (state: Exclude<AndroidEmbedState, "closed">): string => ({
  loading: "Connexion…",
  ready: "En direct",
  error: "À vérifier",
})[state];

const androidEmbedStateIcon = (state: Exclude<AndroidEmbedState, "closed">): string => ({
  loading: "loader-circle",
  ready: "badge-check",
  error: "circle-alert",
})[state];

const renderAndroidScreen = (deviceSerial: string, session: AndroidEmbedSession): string => {
  const details = deviceDetails().find((device) => device.serial === deviceSerial);
  const embedUrl = readyDeviceSerials().includes(deviceSerial)
    ? buildScrcpyEmbedUrl(deviceSerial)
    : null;
  const renderedState = embedUrl ? session.state : "error";
  const renderedError = session.error
    || "L’URL locale de ws-scrcpy-web a été refusée ou l’appareil Android n’est plus disponible.";
  const label = details ? deviceLabel(details) : deviceSerial;

  return `<article class="android-control-screen" data-android-screen data-state="${renderedState}" data-device="${escapeAttr(deviceSerial)}" aria-busy="${renderedState === "loading" ? "true" : "false"}">
    <header class="android-control-screen-header">
      <span class="android-control-screen-copy">
        <strong title="${escapeAttr(label)}">${escapeHtml(label)}</strong>
        <small title="${escapeAttr(deviceSerial)}">${escapeHtml(deviceSerial)}</small>
      </span>
      <span class="android-control-screen-state">
        <i data-lucide="${androidEmbedStateIcon(renderedState)}" class="${renderedState === "loading" ? "is-spinning" : ""}"></i>
        ${androidEmbedStateLabel(renderedState)}
      </span>
      <span class="android-control-screen-actions">
        <button type="button" class="icon-button" data-android-reload-screen data-device="${escapeAttr(deviceSerial)}" title="Recharger ${escapeAttr(label)}" aria-label="Recharger l’écran ${escapeAttr(label)}">
          <i data-lucide="refresh-cw"></i>
        </button>
        <button type="button" class="icon-button" data-android-close-screen data-device="${escapeAttr(deviceSerial)}" title="Fermer ${escapeAttr(label)}" aria-label="Fermer l’écran ${escapeAttr(label)}">
          <i data-lucide="x"></i>
        </button>
      </span>
    </header>
    ${embedUrl
      ? `<div class="android-control-frame-shell">
          <iframe
            class="android-control-embed-frame"
            data-android-embed-frame
            data-device="${escapeAttr(deviceSerial)}"
            src="${escapeAttr(embedUrl)}"
            title="Écran Android ${escapeAttr(label)}"
            sandbox="allow-scripts allow-same-origin"
            allow="fullscreen"
            referrerpolicy="no-referrer"
          ></iframe>
          <div class="android-control-frame-loading" role="status">
            <i data-lucide="loader-circle" class="is-spinning"></i>
            <strong>Connexion au flux…</strong>
            <small>Service local : 127.0.0.1:8000</small>
          </div>
          <div class="android-control-frame-error" role="alert">
            <i data-lucide="circle-alert"></i>
            <strong>Flux indisponible</strong>
            <p data-android-control-embed-error>${escapeHtml(renderedError)}</p>
            <small>Rechargez cet écran ou utilisez le bouton scrcpy externe de secours.</small>
          </div>
        </div>`
      : `<div class="android-control-frame-unavailable" role="alert">
          <i data-lucide="circle-alert"></i>
          <strong>Appareil Android indisponible</strong>
          <p data-android-control-embed-error>${escapeHtml(renderedError)}</p>
        </div>`}
  </article>`;
};

const renderAndroidEmbed = (): string => {
  const selectedSerial = effectiveDevice();
  const serviceState = effectiveWsScrcpyServiceState();
  const sessions = [...androidEmbedSessions.entries()];
  const remainingSlots = Math.max(0, MAX_ANDROID_LIVE_SCREENS - sessions.length);
  const readyUnopened = readyDeviceSerials()
    .filter((deviceSerial) => !androidEmbedSessions.has(deviceSerial));
  const canOpenSelected = serviceState === "online"
    && !!selectedSerial
    && !androidEmbedSessions.has(selectedSerial)
    && remainingSlots > 0
    && buildScrcpyEmbedUrl(selectedSerial) !== null;
  const canOpenAll = serviceState === "online" && readyUnopened.length > 0 && remainingSlots > 0;
  const renderedState = overallAndroidEmbedState();

  return `<section class="android-control-embed" data-android-control-embed data-state="${renderedState}" data-service-state="${serviceState}">
    <header class="android-control-embed-header">
      <span class="android-control-embed-copy">
        <small>ws-scrcpy-web local · supervision</small>
        <strong>Mur d’écrans Android</strong>
        <p>Jusqu’à ${MAX_ANDROID_LIVE_SCREENS} flux simultanés, limités à <code>http://127.0.0.1:8000</code>.</p>
      </span>
      <span class="android-control-embed-status" data-android-embed-summary aria-live="polite">${escapeHtml(androidEmbedSummary())}</span>
      <span class="android-control-embed-actions">
        ${renderWsScrcpyServiceButton()}
        <button type="button" id="androidControlOpenEmbed" class="tool-button" ${canOpenSelected ? "" : "disabled"}>
          <i data-lucide="monitor-up"></i><span>Ajouter la sélection</span>
        </button>
        <button type="button" id="androidControlOpenAllEmbeds" class="tool-button primary" ${canOpenAll ? "" : "disabled"}>
          <i data-lucide="panels-top-left"></i><span>Afficher tous (${Math.min(readyUnopened.length, remainingSlots)})</span>
        </button>
        <button type="button" id="androidControlCloseAllEmbeds" class="tool-button" ${sessions.length ? "" : "disabled"}>
          <i data-lucide="monitor-off"></i><span>Tout fermer</span>
        </button>
      </span>
    </header>
    ${serviceState === "error" && wsScrcpyServiceError
      ? `<div class="android-control-service-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(wsScrcpyServiceError)}</span></div>`
      : ""}
    ${snapshot && !snapshot.wsScrcpyAvailable && serviceState !== "online"
      ? `<div class="android-control-service-help" role="note"><i data-lucide="info"></i><span>ws-scrcpy-web n’est pas encore installé — il sera téléchargé et installé automatiquement au premier démarrage du service.</span></div>`
      : ""}
    ${readyDeviceSerials().length > MAX_ANDROID_LIVE_SCREENS
      ? `<div class="android-control-service-help" role="note"><i data-lucide="info"></i><span>${readyDeviceSerials().length} appareils sont prêts ; les ${MAX_ANDROID_LIVE_SCREENS} premiers sont affichés et les autres restent disponibles après fermeture d’un flux.</span></div>`
      : ""}
    ${sessions.length
      ? `<div class="android-control-screen-grid" data-android-screen-grid>
          ${sessions.map(([deviceSerial, session]) => renderAndroidScreen(deviceSerial, session)).join("")}
        </div>`
      : `<div class="android-control-embed-empty">
          <i data-lucide="panels-top-left"></i>
          <span>
            <strong>Aucun écran ouvert</strong>
            <small>${serviceState === "online"
              ? "Ajoutez l’appareil sélectionné ou affichez tous les appareils ADB prêts, jusqu’à 40."
              : "Démarrez ws-scrcpy-web pour activer le mur d’écrans supervisé."}</small>
          </span>
        </div>`}
  </section>`;
};

export const renderAndroidControlPanel = ({
  remoteMode,
}: AndroidControlPanelOptions): string => {
  if (!remoteMode) {
    return `<div id="androidControlPanel" class="android-control-panel">
      <section class="android-control-unavailable">
        <span><i data-lucide="usb"></i></span>
        <h2>Connexion VPS requise</h2>
        <p>Le contrôle USB local est disponible depuis le PC Windows connecté au VPS. Ouvrez cet onglet dans Switch Cloud après avoir démarré le pont Android sur ce PC.</p>
      </section>
    </div>`;
  }

  const selectedSerial = effectiveDevice();
  const bridgeReady = snapshot?.bridgeOnline === true || snapshot?.connectorOnline === true;
  const externalScrcpyDisabled =
    !bridgeReady
    || !selectedSerial
    || snapshot?.scrcpyAvailable !== true
    || openingExternalScrcpy;

  return `<div id="androidControlPanel" class="android-control-panel">
    <section class="android-control-hero">
      <div class="android-control-hero-copy">
        <span class="android-control-hero-mark"><i data-lucide="smartphone"></i></span>
        <span>
          <small>Android · ADB</small>
          <h2>Contrôler un téléphone dans Switch</h2>
          <p>Sélectionnez l’appareil relié au pont Android, puis ouvrez son écran local sécurisé.</p>
        </span>
      </div>
      <div class="android-control-hero-status">
        ${renderConnectionStatus()}
        <button type="button" id="androidControlRefresh" class="icon-button" title="Actualiser" aria-label="Actualiser les appareils Android" ${loading ? "disabled" : ""}>
          <i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i>
        </button>
      </div>
    </section>

    <div class="android-control-local-note" role="note">
      <i data-lucide="usb"></i>
      <span><strong>Contrôle USB local depuis Windows</strong><small>Le contrôle USB local est disponible depuis le PC Windows connecté au VPS. Comme ws-scrcpy-web ne comporte pas d’authentification intégrée, gardez le port 8000 bloqué en entrée depuis le réseau par le pare-feu Windows.</small></span>
    </div>

    ${error ? `<div class="android-control-feedback is-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(error)}</span></div>` : ""}
    ${feedback ? `<div class="android-control-feedback is-${feedback.tone}" role="status"><i data-lucide="${feedback.tone === "success" ? "badge-check" : feedback.tone === "warning" ? "triangle-alert" : "circle-alert"}"></i><span>${escapeHtml(feedback.message)}</span></div>` : ""}
    ${snapshot?.adbError ? `<div class="android-control-feedback is-warning" role="status"><i data-lucide="triangle-alert"></i><span>${escapeHtml(snapshot.adbError)}</span></div>` : ""}

    <section class="android-control-device-card">
      <header>
        <span><small>Appareil cible</small><strong>Connexion ADB</strong></span>
        ${loading && !snapshot ? `<span class="android-control-device-loading"><i data-lucide="loader-circle" class="is-spinning"></i>Détection…</span>` : ""}
      </header>
      ${renderDevicePicker()}
      <div class="android-control-device-actions">
        <button type="button" id="androidControlOpenScrcpy" class="tool-button" ${externalScrcpyDisabled ? "disabled" : ""}>
          <i data-lucide="${openingExternalScrcpy ? "loader-circle" : "external-link"}" class="${openingExternalScrcpy ? "is-spinning" : ""}"></i>
          <span>${openingExternalScrcpy ? "Ouverture…" : "Ouvrir scrcpy externe · secours"}</span>
        </button>
        ${snapshot && !snapshot.scrcpyAvailable ? `<small>scrcpy n’est pas installé ou CST_SCRCPY_PATH n’est pas configuré sur le PC Windows.</small>` : ""}
      </div>
    </section>

    ${renderAndroidEmbed()}
  </div>`;
};

const scheduleWsScrcpyStartTimeout = (options: AndroidControlUiOptions): void => {
  clearWsScrcpyStartTimer();
  wsScrcpyStartTimer = window.setTimeout(() => {
    wsScrcpyStartTimer = null;
    if (
      wsScrcpyServiceState !== "starting"
      || (snapshot?.bridgeOnline === true && snapshot.wsScrcpyOnline === true)
    ) return;
    wsScrcpyServiceState = "error";
    wsScrcpyServiceError =
      "ws-scrcpy-web n’a pas été détecté sur 127.0.0.1:8000 après le démarrage. Vérifiez son installation et son port local.";
    feedback = { tone: "error", message: wsScrcpyServiceError };
    options.setStatus?.("Démarrage de ws-scrcpy-web non confirmé");
    options.rerender();
  }, WS_SCRCPY_START_TIMEOUT_MS);
};

const startWsScrcpyService = async (options: AndroidControlUiOptions): Promise<void> => {
  if (wsScrcpyServiceState === "starting") return;
  if (snapshot?.bridgeOnline === true && snapshot.wsScrcpyOnline === true) {
    wsScrcpyServiceState = "online";
    feedback = {
      tone: "success",
      message: "ws-scrcpy-web est déjà en ligne sur le PC Windows.",
    };
    options.rerender();
    return;
  }
  if (snapshot?.bridgeOnline !== true && snapshot?.connectorOnline !== true) {
    wsScrcpyServiceState = "error";
    wsScrcpyServiceError = "Le pont Android Windows doit être en ligne pour lancer ws-scrcpy-web.";
    feedback = { tone: "error", message: wsScrcpyServiceError };
    options.rerender();
    return;
  }
  if (snapshot?.wsScrcpyAvailable !== true) {
    // Premier lancement possible : le connecteur Windows provisionne
    // ws-scrcpy-web automatiquement (téléchargement vérifié par SHA-256)
    // si aucun lanceur n'est configuré ni installé.
    feedback = {
      tone: "warning",
      message:
        "ws-scrcpy-web n'est pas encore installé sur le PC Windows — téléchargement et installation automatiques au premier démarrage.",
    };
  }

  wsScrcpyServiceState = "starting";
  wsScrcpyServiceError = "";
  feedback = null;
  error = "";
  scheduleWsScrcpyStartTimeout(options);
  options.rerender();
  try {
    const queued = await invoke<ScrcpySetupAction>("manage_tiktok_sender_login", {
      action: "start_ws_scrcpy",
    });
    if (queued.status === "failed") {
      throw new Error(queued.detail || "Le connecteur Windows a refusé le démarrage de ws-scrcpy-web.");
    }
    if (wsScrcpyServiceState === "starting") {
      feedback = {
        tone: "warning",
        message: "Commande envoyée au PC Windows. Détection de ws-scrcpy-web en cours…",
      };
      options.setStatus?.("Démarrage de ws-scrcpy-web demandé");
      options.rerender();
    }
    void refreshAndroidDevices(options.rerender, { silent: true });
  } catch (cause) {
    clearWsScrcpyStartTimer();
    wsScrcpyServiceState = "error";
    wsScrcpyServiceError = readableError(cause);
    feedback = { tone: "error", message: wsScrcpyServiceError };
    options.setStatus?.("Démarrage de ws-scrcpy-web impossible");
    options.rerender();
  }
};

const openAndroidEmbeds = (
  deviceSerials: string[],
  options: AndroidControlUiOptions,
): void => {
  if (!deviceSerials.length) {
    feedback = {
      tone: "warning",
      message: "Aucun nouvel appareil Android prêt ne peut être ajouté au mur d’écrans.",
    };
    options.rerender();
    return;
  }
  if (effectiveWsScrcpyServiceState() !== "online") {
    feedback = {
      tone: "warning",
      message: "Démarrez d’abord ws-scrcpy-web et attendez que le service soit détecté.",
    };
    options.rerender();
    return;
  }
  const availableSlots = Math.max(0, MAX_ANDROID_LIVE_SCREENS - androidEmbedSessions.size);
  if (!availableSlots) {
    feedback = {
      tone: "warning",
      message: `Le mur supervise déjà ${MAX_ANDROID_LIVE_SCREENS} écrans, sa capacité maximale.`,
    };
    options.rerender();
    return;
  }
  const ready = new Set(readyDeviceSerials());
  const candidates = [...new Set(deviceSerials)]
    .filter((deviceSerial) => ready.has(deviceSerial))
    .filter((deviceSerial) => !androidEmbedSessions.has(deviceSerial))
    .filter((deviceSerial) => buildScrcpyEmbedUrl(deviceSerial) !== null)
    .slice(0, availableSlots);
  if (!candidates.length) {
    feedback = {
      tone: "warning",
      message: "Les appareils demandés sont déjà affichés, indisponibles ou refusés par la règle d’URL locale.",
    };
    options.rerender();
    return;
  }

  for (const deviceSerial of candidates) {
    clearAndroidEmbedLoadTimer(deviceSerial);
    androidEmbedSessions.set(deviceSerial, { state: "loading", error: "" });
  }
  feedback = null;
  options.setStatus?.(
    candidates.length === 1
      ? `Écran Android ${candidates[0]} ajouté`
      : `${candidates.length} écrans Android ajoutés`,
  );
  options.rerender();
};

const openAndroidEmbed = (options: AndroidControlUiOptions): void => {
  const deviceSerial = effectiveDevice();
  if (!deviceSerial) {
    feedback = {
      tone: "warning",
      message: "Choisissez d’abord un appareil Android prêt à afficher dans Switch.",
    };
    options.rerender();
    return;
  }
  openAndroidEmbeds([deviceSerial], options);
};

const openAllAndroidEmbeds = (options: AndroidControlUiOptions): void => {
  openAndroidEmbeds(readyDeviceSerials(), options);
};

const reloadAndroidEmbed = (
  deviceSerial: string,
  options: AndroidControlUiOptions,
): void => {
  if (!androidEmbedSessions.has(deviceSerial)) return;
  clearAndroidEmbedLoadTimer(deviceSerial);
  androidEmbedSessions.set(deviceSerial, { state: "loading", error: "" });
  const frame = [...document.querySelectorAll<HTMLIFrameElement>("[data-android-embed-frame]")]
    .find((candidate) => candidate.dataset.device === deviceSerial);
  const embedUrl = buildScrcpyEmbedUrl(deviceSerial);
  if (!frame || !embedUrl) {
    options.rerender();
    return;
  }
  setAndroidEmbedState(deviceSerial, "loading");
  bindAndroidEmbedFrame(frame);
  frame.src = embedUrl;
};

const openExternalScrcpy = async (options: AndroidControlUiOptions): Promise<void> => {
  if (openingExternalScrcpy) return;
  const deviceSerial = effectiveDevice();
  if (!deviceSerial) {
    feedback = {
      tone: "warning",
      message: "Choisissez d’abord l’appareil Android à ouvrir avec scrcpy.",
    };
    options.rerender();
    return;
  }

  openingExternalScrcpy = true;
  error = "";
  feedback = null;
  options.rerender();
  try {
    const queued = await invoke<ScrcpySetupAction>("manage_tiktok_sender_login", {
      action: "open_scrcpy",
      deviceSerial,
    });
    feedback = {
      tone: "success",
      message: `scrcpy s’ouvre sur ${queued.deviceSerial} depuis le PC Windows connecté au VPS.`,
    };
    options.setStatus?.("Ouverture de scrcpy demandée");
  } catch (cause) {
    feedback = { tone: "error", message: readableError(cause) };
  } finally {
    openingExternalScrcpy = false;
    options.rerender();
  }
};

const bindAndroidEmbedFrame = (frame: HTMLIFrameElement): void => {
  const frameDevice = frame.dataset.device ?? "";
  if (!frameDevice || !androidEmbedSessions.has(frameDevice)) return;
  const ownsCurrentFrame = (): boolean =>
    androidEmbedSessions.has(frameDevice)
    && [...document.querySelectorAll<HTMLIFrameElement>("[data-android-embed-frame]")]
      .some((candidate) => candidate === frame);
  const requestHandshake = (): void => {
    if (!ownsCurrentFrame()) return;
    frame.contentWindow?.postMessage(
      { type: "ws-scrcpy-web:theme-request" },
      SCRCPY_EMBED_ORIGIN,
    );
  };

  clearAndroidEmbedLoadTimer(frameDevice);
  setAndroidEmbedState(frameDevice, "loading");
  const timer = window.setTimeout(() => {
    androidEmbedLoadTimers.delete(frameDevice);
    if (!ownsCurrentFrame()) return;
    setAndroidEmbedState(
      frameDevice,
      "error",
      "Le service local ne répond pas. Démarrez ws-scrcpy-web sur 127.0.0.1:8000, puis rechargez.",
    );
  }, SCRCPY_EMBED_LOAD_TIMEOUT_MS);
  androidEmbedLoadTimers.set(frameDevice, timer);

  frame.addEventListener("load", requestHandshake, { once: true });
  frame.addEventListener("error", () => {
    if (!ownsCurrentFrame()) return;
    setAndroidEmbedState(
      frameDevice,
      "error",
      "Le chargement de ws-scrcpy-web a échoué. Vérifiez le service local, puis rechargez.",
    );
  }, { once: true });
  // Récupère le handshake à usage unique si l’iframe était déjà chargée au binding.
  requestHandshake();
};

const bindAndroidEmbedFrames = (): void => {
  const frames = [...document.querySelectorAll<HTMLIFrameElement>("[data-android-embed-frame]")];
  if (!frames.length) return;
  bindAndroidEmbedMessages();
  for (const frame of frames) bindAndroidEmbedFrame(frame);
};

const updateSelectedDeviceControls = (): void => {
  const selectedSerial = effectiveDevice();
  const bridgeReady = snapshot?.bridgeOnline === true || snapshot?.connectorOnline === true;
  const externalScrcpy = document.querySelector<HTMLButtonElement>("#androidControlOpenScrcpy");
  if (externalScrcpy) {
    externalScrcpy.disabled =
      !bridgeReady
      || !selectedSerial
      || snapshot?.scrcpyAvailable !== true
      || openingExternalScrcpy;
  }
  const addSelected = document.querySelector<HTMLButtonElement>("#androidControlOpenEmbed");
  if (addSelected) {
    addSelected.disabled =
      effectiveWsScrcpyServiceState() !== "online"
      || !selectedSerial
      || androidEmbedSessions.has(selectedSerial)
      || androidEmbedSessions.size >= MAX_ANDROID_LIVE_SCREENS
      || buildScrcpyEmbedUrl(selectedSerial) === null;
  }
};

const updateAndroidEmbedActionControls = (): void => {
  updateSelectedDeviceControls();
  const remainingSlots = Math.max(0, MAX_ANDROID_LIVE_SCREENS - androidEmbedSessions.size);
  const readyUnopened = readyDeviceSerials()
    .filter((deviceSerial) => !androidEmbedSessions.has(deviceSerial));
  const openAll = document.querySelector<HTMLButtonElement>("#androidControlOpenAllEmbeds");
  if (openAll) {
    openAll.disabled =
      effectiveWsScrcpyServiceState() !== "online"
      || !readyUnopened.length
      || !remainingSlots;
    const label = openAll.querySelector("span");
    if (label) label.textContent = `Afficher tous (${Math.min(readyUnopened.length, remainingSlots)})`;
  }
  const closeAll = document.querySelector<HTMLButtonElement>("#androidControlCloseAllEmbeds");
  if (closeAll) closeAll.disabled = androidEmbedSessions.size === 0;
};

export const bindAndroidControlUi = (options: AndroidControlUiOptions): void => {
  panelSetStatus = options.setStatus ?? null;
  document.querySelector<HTMLSelectElement>("#androidControlDevice")?.addEventListener("change", (event) => {
    selectedDevice = (event.currentTarget as HTMLSelectElement).value;
    feedback = null;
    updateAndroidEmbedActionControls();
  });
  document.querySelector<HTMLButtonElement>("#androidControlRefresh")?.addEventListener("click", () => {
    void refreshAndroidDevices(options.rerender);
  });
  document.querySelector<HTMLButtonElement>("#androidControlOpenEmbed")?.addEventListener("click", () => {
    openAndroidEmbed(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlOpenAllEmbeds")?.addEventListener("click", () => {
    openAllAndroidEmbeds(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlCloseAllEmbeds")?.addEventListener("click", () => {
    resetAndroidEmbed();
    options.rerender();
  });
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-android-reload-screen]")) {
    button.addEventListener("click", () => {
      reloadAndroidEmbed(button.dataset.device ?? "", options);
    });
  }
  for (const button of document.querySelectorAll<HTMLButtonElement>("[data-android-close-screen]")) {
    button.addEventListener("click", () => {
      const deviceSerial = button.dataset.device ?? "";
      resetAndroidEmbed(deviceSerial);
      const screen = [...document.querySelectorAll<HTMLElement>("[data-android-screen]")]
        .find((candidate) => candidate.dataset.device === deviceSerial);
      screen?.remove();
      if (!androidEmbedSessions.size) {
        options.rerender();
        return;
      }
      updateAndroidEmbedSummaryDom();
      updateAndroidEmbedActionControls();
    });
  }
  document.querySelector<HTMLButtonElement>("#androidControlOpenScrcpy")?.addEventListener("click", () => {
    void openExternalScrcpy(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlStartWebScrcpy")?.addEventListener("click", () => {
    void startWsScrcpyService(options);
  });
  bindAndroidEmbedFrames();
};

export const activateAndroidControlPanel = (
  rerender: () => void,
  remoteMode = true,
): void => {
  visible = true;
  panelRerender = rerender;
  if (!remoteMode) {
    stopPolling();
    return;
  }

  if (pollTimer === null) {
    pollTimer = window.setInterval(() => {
      if (visible && panelRerender) {
        void refreshAndroidDevices(panelRerender, { silent: true });
      }
    }, POLL_INTERVAL_MS);
  }
  void refreshAndroidDevices(rerender, { silent: snapshot !== null });
};

export const deactivateAndroidControlPanel = (): void => {
  visible = false;
  panelRerender = null;
  panelSetStatus = null;
  stopPolling();
  clearAndroidEmbedLoadTimer();
  clearWsScrcpyStartTimer();
  if (wsScrcpyServiceState === "starting") wsScrcpyServiceState = "idle";
  unbindAndroidEmbedMessages();
};
