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
type WsScrcpyServiceState = "idle" | "starting" | "online" | "error";

const POLL_INTERVAL_MS = 4_000;
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
let androidEmbedState: AndroidEmbedState = "closed";
let androidEmbedDevice = "";
let androidEmbedError = "";
let androidEmbedLoadTimer: number | null = null;
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

const clearAndroidEmbedLoadTimer = (): void => {
  if (androidEmbedLoadTimer === null) return;
  window.clearTimeout(androidEmbedLoadTimer);
  androidEmbedLoadTimer = null;
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

const resetAndroidEmbed = (): void => {
  clearAndroidEmbedLoadTimer();
  androidEmbedState = "closed";
  androidEmbedDevice = "";
  androidEmbedError = "";
};

const setAndroidEmbedState = (
  state: Exclude<AndroidEmbedState, "closed">,
  nextError = "",
): void => {
  if (state !== "loading") clearAndroidEmbedLoadTimer();
  androidEmbedState = state;
  androidEmbedError = nextError;

  const panel = document.querySelector<HTMLElement>("[data-android-control-embed]");
  if (!panel || panel.dataset.device !== androidEmbedDevice) return;
  panel.dataset.state = state;
  panel.setAttribute("aria-busy", state === "loading" ? "true" : "false");
  const errorCopy = panel.querySelector<HTMLElement>("[data-android-control-embed-error]");
  if (errorCopy) errorCopy.textContent = nextError;
};

const androidEmbedTheme = (): "dark" | "light" =>
  document.documentElement.dataset.theme === "light" ? "light" : "dark";

const handleAndroidEmbedMessage = (event: MessageEvent): void => {
  if (event.origin !== SCRCPY_EMBED_ORIGIN) return;
  const frame = document.querySelector<HTMLIFrameElement>("#androidControlEmbedFrame");
  if (
    !frame
    || event.source !== frame.contentWindow
    || frame.dataset.device !== androidEmbedDevice
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
  setAndroidEmbedState("ready");
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
  if (!androidEmbedDevice || readyDeviceSerials().includes(androidEmbedDevice)) return;
  clearAndroidEmbedLoadTimer();
  androidEmbedState = "error";
  androidEmbedError =
    `L’appareil ${androidEmbedDevice} n’est plus disponible. Fermez cet écran et choisissez un appareil Android prêt.`;
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
    if (androidEmbedState !== "closed") {
      setAndroidEmbedState(
        "error",
        "Le service local ws-scrcpy-web ne répond plus. Redémarrez-le puis rechargez l’écran.",
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

const renderAndroidEmbed = (): string => {
  const selectedSerial = effectiveDevice();
  const selectedDetails = deviceDetails().find((device) => device.serial === selectedSerial);
  const isOpen = androidEmbedState !== "closed" && !!androidEmbedDevice;
  const embedDeviceReady = readyDeviceSerials().includes(androidEmbedDevice);
  const embedUrl = isOpen && embedDeviceReady
    ? buildScrcpyEmbedUrl(androidEmbedDevice)
    : null;
  const renderedState: AndroidEmbedState = !isOpen
    ? "closed"
    : embedUrl
      ? androidEmbedState
      : "error";
  const renderedError = androidEmbedError
    || "L’URL locale de ws-scrcpy-web a été refusée ou l’appareil Android n’est plus disponible.";
  const serviceState = effectiveWsScrcpyServiceState();
  const canOpen = serviceState === "online"
    && !!selectedSerial
    && buildScrcpyEmbedUrl(selectedSerial) !== null;
  const activeLabel = androidEmbedDevice || selectedSerial;

  return `<section class="android-control-embed" data-android-control-embed data-state="${renderedState}" data-service-state="${serviceState}" data-device="${escapeAttr(androidEmbedDevice)}" aria-busy="${renderedState === "loading" ? "true" : "false"}">
    <header class="android-control-embed-header">
      <span class="android-control-embed-copy">
        <small>ws-scrcpy-web local</small>
        <strong>Écran Android intégré</strong>
        <p>L’iframe est limitée à <code>http://127.0.0.1:8000</code>.</p>
      </span>
      <span class="android-control-embed-status" aria-live="polite">
        <span data-android-embed-status="closed"><i data-lucide="wifi-off"></i>Écran fermé</span>
        <span data-android-embed-status="loading"><i data-lucide="loader-circle" class="is-spinning"></i>Connexion locale…</span>
        <span data-android-embed-status="ready"><i data-lucide="badge-check"></i>Service local chargé</span>
        <span data-android-embed-status="error"><i data-lucide="circle-alert"></i>Service indisponible</span>
      </span>
      <span class="android-control-embed-actions">
        ${renderWsScrcpyServiceButton()}
        ${isOpen
          ? `<button type="button" id="androidControlReloadEmbed" class="tool-button">
              <i data-lucide="refresh-cw"></i><span>Recharger</span>
            </button>
            <button type="button" id="androidControlCloseEmbed" class="tool-button">
              <i data-lucide="wifi-off"></i><span>Fermer</span>
            </button>`
          : `<button type="button" id="androidControlOpenEmbed" class="tool-button primary" ${canOpen ? "" : "disabled"}>
              <i data-lucide="smartphone"></i><span>Afficher dans Switch</span>
            </button>`}
      </span>
    </header>
    ${serviceState === "error" && wsScrcpyServiceError
      ? `<div class="android-control-service-error" role="alert"><i data-lucide="circle-alert"></i><span>${escapeHtml(wsScrcpyServiceError)}</span></div>`
      : ""}
    ${snapshot && !snapshot.wsScrcpyAvailable && serviceState !== "online"
      ? `<div class="android-control-service-help" role="note"><i data-lucide="info"></i><span>ws-scrcpy-web n’est pas encore installé — il sera téléchargé et installé automatiquement au premier démarrage du service.</span></div>`
      : ""}
    ${!isOpen
      ? `<div class="android-control-embed-empty">
          <i data-lucide="smartphone"></i>
          <span>
            <strong>${selectedDetails ? escapeHtml(deviceLabel(selectedDetails)) : "Choisissez un appareil Android prêt"}</strong>
            <small>${selectedSerial
              ? serviceState === "online"
                ? escapeHtml(selectedSerial)
                : "Démarrez ws-scrcpy-web pour activer l’affichage intégré."
              : "Le bouton s’activera dès qu’un appareil ADB sera sélectionné et que le service sera en ligne."}</small>
          </span>
        </div>`
      : embedUrl
        ? `<div class="android-control-frame-shell">
            <iframe
              id="androidControlEmbedFrame"
              data-device="${escapeAttr(androidEmbedDevice)}"
              src="${escapeAttr(embedUrl)}"
              title="Écran Android ${escapeAttr(activeLabel)}"
              sandbox="allow-scripts allow-same-origin"
              allow="fullscreen"
              referrerpolicy="no-referrer"
            ></iframe>
            <div class="android-control-frame-loading" role="status">
              <i data-lucide="loader-circle" class="is-spinning"></i>
              <strong>Connexion à ws-scrcpy-web…</strong>
              <small>Service local : 127.0.0.1:8000</small>
            </div>
            <div class="android-control-frame-error" role="alert">
              <i data-lucide="circle-alert"></i>
              <strong>Impossible d’afficher l’appareil</strong>
              <p data-android-control-embed-error>${escapeHtml(renderedError)}</p>
              <small>Rechargez le panneau ou utilisez le bouton scrcpy externe de secours.</small>
            </div>
          </div>`
        : `<div class="android-control-frame-unavailable" role="alert">
            <i data-lucide="circle-alert"></i>
            <strong>Appareil Android indisponible</strong>
            <p data-android-control-embed-error>${escapeHtml(renderedError)}</p>
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
  if (effectiveWsScrcpyServiceState() !== "online") {
    feedback = {
      tone: "warning",
      message: "Démarrez d’abord ws-scrcpy-web et attendez que le service soit détecté.",
    };
    options.rerender();
    return;
  }
  if (!buildScrcpyEmbedUrl(deviceSerial)) {
    feedback = {
      tone: "error",
      message: "L’URL locale ws-scrcpy-web a été refusée pour des raisons de sécurité.",
    };
    options.rerender();
    return;
  }

  clearAndroidEmbedLoadTimer();
  androidEmbedDevice = deviceSerial;
  androidEmbedState = "loading";
  androidEmbedError = "";
  feedback = null;
  options.rerender();
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

const bindAndroidEmbedFrame = (): void => {
  const frame = document.querySelector<HTMLIFrameElement>("#androidControlEmbedFrame");
  if (!frame) return;
  bindAndroidEmbedMessages();

  const frameDevice = frame.dataset.device ?? "";
  const ownsCurrentFrame = (): boolean =>
    frameDevice === androidEmbedDevice
    && document.querySelector("#androidControlEmbedFrame") === frame;
  const requestHandshake = (): void => {
    if (!ownsCurrentFrame()) return;
    frame.contentWindow?.postMessage(
      { type: "ws-scrcpy-web:theme-request" },
      SCRCPY_EMBED_ORIGIN,
    );
  };

  clearAndroidEmbedLoadTimer();
  setAndroidEmbedState("loading");
  androidEmbedLoadTimer = window.setTimeout(() => {
    if (!ownsCurrentFrame()) return;
    setAndroidEmbedState(
      "error",
      "Le service local ne répond pas. Démarrez ws-scrcpy-web sur 127.0.0.1:8000, puis rechargez.",
    );
  }, SCRCPY_EMBED_LOAD_TIMEOUT_MS);

  frame.addEventListener("load", requestHandshake, { once: true });
  frame.addEventListener("error", () => {
    if (!ownsCurrentFrame()) return;
    setAndroidEmbedState(
      "error",
      "Le chargement de ws-scrcpy-web a échoué. Vérifiez le service local, puis rechargez.",
    );
  }, { once: true });
  // Récupère le handshake à usage unique si l’iframe était déjà chargée au binding.
  requestHandshake();
};

export const bindAndroidControlUi = (options: AndroidControlUiOptions): void => {
  panelSetStatus = options.setStatus ?? null;
  document.querySelector<HTMLSelectElement>("#androidControlDevice")?.addEventListener("change", (event) => {
    selectedDevice = (event.currentTarget as HTMLSelectElement).value;
    if (androidEmbedDevice && androidEmbedDevice !== selectedDevice) resetAndroidEmbed();
    feedback = null;
    options.rerender();
  });
  document.querySelector<HTMLButtonElement>("#androidControlRefresh")?.addEventListener("click", () => {
    void refreshAndroidDevices(options.rerender);
  });
  document.querySelector<HTMLButtonElement>("#androidControlOpenEmbed")?.addEventListener("click", () => {
    openAndroidEmbed(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlReloadEmbed")?.addEventListener("click", () => {
    openAndroidEmbed(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlCloseEmbed")?.addEventListener("click", () => {
    resetAndroidEmbed();
    options.rerender();
  });
  document.querySelector<HTMLButtonElement>("#androidControlOpenScrcpy")?.addEventListener("click", () => {
    void openExternalScrcpy(options);
  });
  document.querySelector<HTMLButtonElement>("#androidControlStartWebScrcpy")?.addEventListener("click", () => {
    void startWsScrcpyService(options);
  });
  bindAndroidEmbedFrame();
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
