import { invoke } from "./platform";
import {
  USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR,
  USB_DEVICE_PANEL_READY_EVENT,
  registerManualUsbDeviceActionHandler,
  type ManualUsbDeviceActionRequest,
} from "./usb-devices";

export const DEVICE_CONTROL_ACTIONS = [
  "info",
  "screenshot",
  "open_screen",
  "tap",
  "swipe",
  "type_text",
  "key_event",
  "open_app",
  "shell",
  "push_file",
] as const;

export type DeviceControlAction = (typeof DEVICE_CONTROL_ACTIONS)[number];
export type DevicePlatform = "android" | "ios" | "unknown";
export type DeviceConnectionState =
  | "ready"
  | "unauthorized"
  | "offline"
  | "busy"
  | "error"
  | "unknown";

export type DeviceControlRequest = {
  deviceId: string;
  action: DeviceControlAction;
  args?: Record<string, unknown>;
  confirmed?: boolean;
};

export type DeviceFleetDevice = {
  id: string;
  key: string;
  platform: DevicePlatform;
  name: string;
  model: string | null;
  serial: string;
  osVersion: string | null;
  transport: string;
  state: DeviceConnectionState;
  stateLabel: string;
  ready: boolean;
  batteryLevel: number | null;
  owner: string | null;
  capabilities: DeviceControlAction[] | null;
};

export type DeviceFleetTool = {
  id: string;
  label: string;
  available: boolean | null;
  detail: string | null;
};

export type DeviceFleetSnapshot = {
  devices: DeviceFleetDevice[];
  bridgeOnline: boolean | null;
  updatedAt: number | null;
  warning: string | null;
  tools: DeviceFleetTool[];
};

type DeviceFleetFilter = "all" | "android" | "ios" | "ready";
type FeedbackTone = "success" | "warning" | "error";
type DeviceActionFeedback = {
  action: DeviceControlAction;
  tone: FeedbackTone;
  message: string;
  result?: unknown;
};
type DeviceMedia = {
  kind: "screen" | "screenshot";
  url: string;
};
type DeviceDraft = {
  tapX: string;
  tapY: string;
  swipeStartX: string;
  swipeStartY: string;
  swipeEndX: string;
  swipeEndY: string;
  swipeDurationMs: string;
  text: string;
  key: string;
  appId: string;
  shell: string;
  localPath: string;
  remotePath: string;
};
type DeviceFleetBindings = {
  rerender: () => void;
  setStatus?: (message: string) => void;
};
type DeviceFleetRenderOptions = {
  remoteMode: boolean;
};

const DEVICE_FLEET_POLL_MS = 4_000;
const MAX_TEXT_LENGTH = 4_000;
const MAX_COMMAND_LENGTH = 4_096;
const MAX_PATH_LENGTH = 4_096;
const MAX_RESULT_TEXT_LENGTH = 12_000;
const MAX_DATA_IMAGE_LENGTH = 20_000_000;
const DEVICE_ACTION_SET = new Set<string>(DEVICE_CONTROL_ACTIONS);
const DEVICE_TOOL_LABELS: Record<string, string> = {
  adb: "ADB",
  scrcpy: "scrcpy",
  ideviceId: "idevice_id",
  ideviceInfo: "ideviceinfo",
  ideviceScreenshot: "idevicescreenshot",
  iproxy: "iproxy",
  ssh: "SSH",
  iosSshKeyConfigured: "Clé SSH iOS",
  iosUiToolConfigured: "Pilotage UI iOS",
};
const DEFAULT_DRAFT: DeviceDraft = {
  tapX: "",
  tapY: "",
  swipeStartX: "",
  swipeStartY: "",
  swipeEndX: "",
  swipeEndY: "",
  swipeDurationMs: "350",
  text: "",
  key: "HOME",
  appId: "",
  shell: "",
  localPath: "",
  remotePath: "",
};

let snapshot: DeviceFleetSnapshot | null = null;
let snapshotSignature = "";
let loading = false;
let loadError = "";
let active = false;
let pollTimer: number | null = null;
let refreshPromise: Promise<boolean> | null = null;
let bindings: DeviceFleetBindings | null = null;
let activeFilter: DeviceFleetFilter = "all";
let releaseAssistedActionHandler: (() => void) | null = null;
const inFlightActions = new Map<string, DeviceControlAction>();
const actionFeedback = new Map<string, DeviceActionFeedback>();
const deviceMedia = new Map<string, DeviceMedia>();
const drafts = new Map<string, DeviceDraft>();
const pendingShellConfirmations = new Map<string, string>();
type PendingPushFileConfirmation = {
  localPath: string;
  remotePath: string;
};
const pendingPushFileConfirmations = new Map<string, PendingPushFileConfirmation>();
const expandedControls = new Set<string>();

const asRecord = (value: unknown): Record<string, unknown> | null =>
  value !== null && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : null;

const textValue = (...values: unknown[]): string => {
  for (const value of values) {
    if (typeof value === "string" && value.trim()) return value.trim();
    if (typeof value === "number" && Number.isFinite(value)) return String(value);
  }
  return "";
};

const booleanValue = (...values: unknown[]): boolean | null => {
  for (const value of values) {
    if (typeof value === "boolean") return value;
    if (value === 1 || value === "1" || value === "true") return true;
    if (value === 0 || value === "0" || value === "false") return false;
  }
  return null;
};

const numberValue = (...values: unknown[]): number | null => {
  for (const value of values) {
    const parsed = typeof value === "number" ? value : Number(value);
    if (Number.isFinite(parsed)) return parsed;
  }
  return null;
};

const escapeHtml = (value: string): string =>
  value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#039;");

const escapeAttr = escapeHtml;

const errorMessage = (error: unknown): string =>
  String(error instanceof Error ? error.message : error).trim()
  || "Une erreur inconnue est survenue.";

const normalizePlatform = (...values: unknown[]): DevicePlatform => {
  const value = textValue(...values).toLowerCase();
  if (/android|adb/.test(value)) return "android";
  if (/ios|iphone|ipad|usbmux|libimobile/.test(value)) return "ios";
  return "unknown";
};

const normalizeState = (
  rawState: string,
  readyValue: boolean | null,
  connectedValue: boolean | null,
  authorizedValue: boolean | null,
): DeviceConnectionState => {
  const state = rawState.toLowerCase();
  if (authorizedValue === false || /unauthorized|untrusted|pair|authoriz/.test(state)) {
    return "unauthorized";
  }
  if (connectedValue === false || /offline|disconnected|missing|unavailable/.test(state)) {
    return "offline";
  }
  if (/busy|locked|leased|occupied|in[_ -]?use/.test(state)) return "busy";
  if (/error|failed|broken/.test(state)) return "error";
  if (readyValue === true || connectedValue === true || /ready|device|online|connected/.test(state)) {
    return "ready";
  }
  return "unknown";
};

const defaultStateLabel = (state: DeviceConnectionState): string => ({
  ready: "Prêt",
  unauthorized: "Autorisation requise",
  offline: "Hors ligne",
  busy: "Occupé",
  error: "Erreur",
  unknown: "État inconnu",
})[state];

const normalizeCapabilities = (value: unknown): DeviceControlAction[] | null => {
  if (Array.isArray(value)) {
    return [...new Set(value.flatMap((item) => {
      const action = textValue(item).toLowerCase();
      return DEVICE_ACTION_SET.has(action) ? [action as DeviceControlAction] : [];
    }))];
  }
  const record = asRecord(value);
  if (!record) return null;
  return DEVICE_CONTROL_ACTIONS.filter((action) => record[action] === true);
};

const normalizeOwner = (value: unknown): string | null => {
  if (typeof value === "string" && value.trim()) return value.trim();
  const record = asRecord(value);
  const label = textValue(record?.label, record?.name, record?.id, record?.ownerId);
  return label || null;
};

const normalizeTools = (value: unknown): DeviceFleetTool[] => {
  const record = asRecord(value);
  if (!record) return [];
  return Object.entries(record).slice(0, 24).map(([id, rawStatus]) => {
    const status = asRecord(rawStatus);
    const available = status
      ? booleanValue(status.available, status.ready, status.configured, status.enabled)
      : booleanValue(rawStatus);
    return {
      id,
      label: DEVICE_TOOL_LABELS[id]
        ?? id.replace(/([a-z0-9])([A-Z])/g, "$1 $2").replaceAll("_", " "),
      available,
      detail: textValue(status?.detail, status?.message, status?.path) || null,
    };
  });
};

export const deviceFleetKey = (platform: DevicePlatform, deviceId: string): string =>
  `${platform}:${deviceId.trim()}`;

const normalizeDevice = (value: unknown): DeviceFleetDevice | null => {
  const device = asRecord(value);
  if (!device) return null;
  const details = asRecord(device.details) ?? asRecord(device.info);
  const platform = normalizePlatform(
    device.platform,
    device.os,
    device.deviceType,
    device.kind,
    details?.platform,
  );
  const id = textValue(device.deviceId, device.id, device.serial, device.udid, details?.id);
  if (!id) return null;
  const serial = textValue(device.serial, device.udid, details?.serial, id);
  const rawState = textValue(
    device.state,
    device.status,
    device.connectionState,
    details?.state,
  );
  const readyValue = booleanValue(device.ready, details?.ready);
  const connectedValue = booleanValue(device.connected, device.online, details?.connected);
  const authorizedValue = booleanValue(device.authorized, device.trusted, details?.authorized);
  const state = normalizeState(rawState, readyValue, connectedValue, authorizedValue);
  const battery = numberValue(device.batteryLevel, device.battery, details?.batteryLevel);
  const capabilitiesSource = device.capabilities ?? device.supportedActions ?? device.actions;
  const model = textValue(device.model, device.product, details?.model) || null;
  const name = textValue(device.name, device.label, device.displayName, model, serial, id);
  return {
    id,
    key: deviceFleetKey(platform, id),
    platform,
    name,
    model,
    serial,
    osVersion: textValue(
      device.osVersion,
      device.version,
      details?.osVersion,
      details?.version,
    ) || null,
    transport: textValue(device.transport, device.connection, details?.transport, "usb"),
    state,
    stateLabel: rawState || defaultStateLabel(state),
    ready: readyValue ?? state === "ready",
    batteryLevel: battery === null ? null : Math.max(0, Math.min(100, Math.round(battery))),
    owner: normalizeOwner(device.owner ?? device.lease ?? device.controlledBy),
    capabilities: normalizeCapabilities(capabilitiesSource),
  };
};

export const normalizeDeviceFleetSnapshot = (value: unknown): DeviceFleetSnapshot => {
  const outer = asRecord(value);
  const data = asRecord(outer?.data);
  const root = data ?? outer;
  const rawDevices = Array.isArray(value)
    ? value
    : Array.isArray(root?.devices)
      ? root.devices
      : Array.isArray(root?.items)
        ? root.items
        : Array.isArray(root?.results)
          ? root.results
          : [];
  const devicesByKey = new Map<string, DeviceFleetDevice>();
  rawDevices.forEach((item) => {
    const device = normalizeDevice(item);
    if (device) devicesByKey.set(device.key, device);
  });
  const devices = [...devicesByKey.values()].sort((left, right) => {
    const platformRank = { android: 0, ios: 1, unknown: 2 } as const;
    return platformRank[left.platform] - platformRank[right.platform]
      || left.name.localeCompare(right.name, "fr", { sensitivity: "base" });
  });
  const rawUpdatedAt = numberValue(
    root?.lastSeenAt,
    root?.updatedAt,
    root?.generatedAt,
    outer?.lastSeenAt,
    outer?.updatedAt,
  );
  const warning = textValue(root?.warning, root?.message, root?.error) || null;
  return {
    devices,
    bridgeOnline: booleanValue(root?.bridgeOnline, root?.online, root?.connectorOnline),
    updatedAt: rawUpdatedAt === null
      ? null
      : rawUpdatedAt < 10_000_000_000 ? rawUpdatedAt * 1_000 : rawUpdatedAt,
    warning,
    tools: normalizeTools(root?.tools),
  };
};

export const buildDeviceActionRequest = (
  deviceId: string,
  action: DeviceControlAction,
  args?: Record<string, unknown>,
  confirmed?: boolean,
): DeviceControlRequest => {
  const normalizedId = deviceId.trim();
  if (!normalizedId) throw new Error("Identifiant d’appareil requis.");
  if (!DEVICE_ACTION_SET.has(action)) throw new Error("Action appareil inconnue.");
  return {
    deviceId: normalizedId,
    action,
    ...(args && Object.keys(args).length ? { args } : {}),
    ...(confirmed === undefined ? {} : { confirmed }),
  };
};

const defaultBaseOrigin = (): string =>
  typeof window !== "undefined" && window.location?.origin
    ? window.location.origin
    : "https://switch.invalid";

export const safeDeviceMediaUrl = (
  value: unknown,
  baseOrigin = defaultBaseOrigin(),
  kind: DeviceMedia["kind"] = "screenshot",
): string | null => {
  if (typeof value !== "string" || !value || value !== value.trim()) return null;
  if (value.length > MAX_DATA_IMAGE_LENGTH) return null;
  if (/^data:image\/(?:png|jpe?g|webp|gif);base64,[a-z0-9+/=\r\n]+$/i.test(value)) {
    return kind === "screenshot" ? value : null;
  }
  let base: URL;
  let url: URL;
  try {
    base = new URL(baseOrigin);
    url = new URL(value, base);
  } catch {
    return null;
  }
  if (url.username || url.password || url.hash) return null;
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  const host = url.hostname.toLowerCase();
  const sameOrigin = url.origin === base.origin;
  if (sameOrigin) {
    const safeId = "[A-Za-z0-9._~-]+";
    const allowedPath = kind === "screen"
      ? new RegExp(`^/api/device-fleet/screens/${safeId}$`)
      : new RegExp(`^/api/device-fleet/(?:screenshots/${safeId}|screens/${safeId}/screenshot)$`);
    return !url.search && allowedPath.test(url.pathname) ? url.href : null;
  }
  if (kind !== "screen") return null;
  const loopback = host === "127.0.0.1" || host === "localhost";
  const queryKeys = [...url.searchParams.keys()];
  const device = url.searchParams.get("device") ?? "";
  const safeDevice = /^[A-Za-z0-9._:~-]{1,512}$/.test(device);
  return loopback
    && url.protocol === "http:"
    && url.port === "8000"
    && url.pathname === "/embed.html"
    && queryKeys.length === 1
    && queryKeys[0] === "device"
    && url.searchParams.getAll("device").length === 1
    && safeDevice
    ? url.href
    : null;
};

const supportsAction = (device: DeviceFleetDevice, action: DeviceControlAction): boolean =>
  device.capabilities === null || device.capabilities.includes(action);

const deviceByKey = (key: string): DeviceFleetDevice | null =>
  snapshot?.devices.find((device) => device.key === key) ?? null;

const draftFor = (key: string): DeviceDraft => {
  const existing = drafts.get(key);
  if (existing) return existing;
  const draft = { ...DEFAULT_DRAFT };
  drafts.set(key, draft);
  return draft;
};

const cleanupRemovedDeviceState = (devices: readonly DeviceFleetDevice[]) => {
  const keys = new Set(devices.map((device) => device.key));
  [actionFeedback, deviceMedia, drafts, pendingShellConfirmations, pendingPushFileConfirmations].forEach((state) => {
    [...state.keys()].forEach((key) => {
      if (!keys.has(key)) state.delete(key);
    });
  });
  [...expandedControls].forEach((key) => {
    if (!keys.has(key)) expandedControls.delete(key);
  });
};

const requestRerender = () => {
  if (active) bindings?.rerender();
};

const refreshDeviceFleet = (options: { silent?: boolean } = {}): Promise<boolean> => {
  if (refreshPromise) return refreshPromise;
  const silent = options.silent === true;
  const task = (async () => {
    loading = true;
    if (!silent) requestRerender();
    try {
      const next = normalizeDeviceFleetSnapshot(await invoke<unknown>("list_control_devices"));
      const signature = JSON.stringify(next);
      const changed = signature !== snapshotSignature || loadError !== "";
      snapshot = next;
      snapshotSignature = signature;
      loadError = "";
      cleanupRemovedDeviceState(next.devices);
      return changed;
    } catch (error) {
      const nextError = errorMessage(error);
      const changed = nextError !== loadError;
      loadError = nextError;
      return changed;
    } finally {
      loading = false;
    }
  })();
  refreshPromise = task;
  const finish = (changed: boolean) => {
    if (refreshPromise === task) refreshPromise = null;
    if (changed || !silent) requestRerender();
    return changed;
  };
  return task.then(finish, (error) => {
    if (refreshPromise === task) refreshPromise = null;
    throw error;
  });
};

const responseRecords = (value: unknown): Record<string, unknown>[] => {
  const first = asRecord(value);
  if (!first) return [];
  return [first, asRecord(first.result), asRecord(first.data)].filter(
    (record): record is Record<string, unknown> => record !== null,
  );
};

const responseMessage = (value: unknown, action: DeviceControlAction): string => {
  if (typeof value === "string" && value.trim()) return value.trim();
  for (const record of responseRecords(value)) {
    const message = textValue(record.message, record.detail, record.statusText, record.output);
    if (message) return message;
    const status = textValue(record.status, record.state).toLowerCase();
    if (/queued|claimed|pending|running/.test(status)) {
      return "Action acceptée par le connecteur, résultat en attente.";
    }
  }
  return ({
    info: "Informations actualisées.",
    screenshot: "Capture d’écran demandée.",
    open_screen: "Ouverture de l’écran demandée.",
    tap: "Appui envoyé.",
    swipe: "Balayage envoyé.",
    type_text: "Texte envoyé.",
    key_event: "Touche envoyée.",
    open_app: "Ouverture de l’application demandée.",
    shell: "Commande shell exécutée.",
    push_file: "Transfert du fichier demandé.",
  })[action];
};

const responseTone = (value: unknown): FeedbackTone => {
  for (const record of responseRecords(value)) {
    const success = booleanValue(record.success, record.ok);
    if (success === false) return "error";
    const status = textValue(record.status, record.state).toLowerCase();
    if (/failed|error|expired|rejected/.test(status)) return "error";
    if (/queued|claimed|pending|running/.test(status)) return "warning";
  }
  return "success";
};

const mediaFromResponse = (value: unknown, action: DeviceControlAction): DeviceMedia | null => {
  for (const record of responseRecords(value)) {
    if (action === "screenshot") {
      const direct = record.screenshotUrl ?? record.imageUrl ?? record.screenshot ?? record.dataUrl;
      const directUrl = safeDeviceMediaUrl(direct, defaultBaseOrigin(), "screenshot");
      if (directUrl) return { kind: "screenshot", url: directUrl };
      const base64 = textValue(
        record.dataBase64,
        record.imageBase64,
        record.screenshotBase64,
        record.base64,
      );
      const mimeType = textValue(record.mimeType, record.contentType, "image/png").toLowerCase();
      if (base64 && /^image\/(?:png|jpe?g|webp|gif)$/.test(mimeType)) {
        const dataUrl = safeDeviceMediaUrl(
          `data:${mimeType};base64,${base64}`,
          defaultBaseOrigin(),
          "screenshot",
        );
        if (dataUrl) return { kind: "screenshot", url: dataUrl };
      }
    }
    if (action === "open_screen") {
      const screenUrl = safeDeviceMediaUrl(
        record.screenUrl ?? record.streamUrl ?? record.url,
        defaultBaseOrigin(),
        "screen",
      );
      if (screenUrl && !screenUrl.startsWith("data:")) return { kind: "screen", url: screenUrl };
    }
  }
  return null;
};

const displayResult = (value: unknown): string => {
  if (value === undefined || value === null) return "";
  try {
    const serialized = typeof value === "string"
      ? value
      : JSON.stringify(value, (_key, item) => {
          if (typeof item === "string" && item.length > 2_000) {
            return `${item.slice(0, 2_000)}… (${item.length} caractères)`;
          }
          return item;
        }, 2);
    if (!serialized) return "";
    return serialized.length > MAX_RESULT_TEXT_LENGTH
      ? `${serialized.slice(0, MAX_RESULT_TEXT_LENGTH)}\n… résultat tronqué`
      : serialized;
  } catch {
    return errorMessage(value);
  }
};

const queuedActionIdentity = (value: unknown): { id: string; pending: boolean } | null => {
  for (const record of responseRecords(value)) {
    const id = textValue(record.id, record.actionId);
    const status = textValue(record.status, record.state).toLowerCase();
    if (id && /queued|claimed|pending|running/.test(status)) {
      return { id, pending: true };
    }
    if (id && /succeeded|failed|expired|rejected|completed/.test(status)) {
      return { id, pending: false };
    }
  }
  return null;
};

const waitForQueuedDeviceAction = async (initial: unknown): Promise<unknown> => {
  let current = initial;
  const first = queuedActionIdentity(current);
  if (!first?.pending) return current;
  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    await new Promise<void>((resolve) => window.setTimeout(resolve, 750));
    current = await invoke<unknown>("get_control_device_action", { actionId: first.id });
    const state = queuedActionIdentity(current);
    if (!state?.pending) return current;
  }
  return current;
};

const performDeviceAction = async (
  key: string,
  action: DeviceControlAction,
  args?: Record<string, unknown>,
  confirmed?: boolean,
) => {
  const device = deviceByKey(key);
  if (!device || inFlightActions.has(key)) return;
  if (!supportsAction(device, action)) {
    actionFeedback.set(key, {
      action,
      tone: "warning",
      message: "Cette action n’est pas annoncée par le contrôleur de cet appareil.",
    });
    requestRerender();
    return;
  }
  inFlightActions.set(key, action);
  actionFeedback.delete(key);
  requestRerender();
  try {
    const request = buildDeviceActionRequest(device.id, action, args, confirmed);
    const accepted = await invoke<unknown>("control_device", request);
    const result = await waitForQueuedDeviceAction(accepted);
    const tone = responseTone(result);
    const media = mediaFromResponse(result, action);
    if (media && tone === "success") deviceMedia.set(key, media);
    actionFeedback.set(key, {
      action,
      tone,
      message: responseMessage(result, action),
      result,
    });
    bindings?.setStatus?.(`${device.name} · ${responseMessage(result, action)}`);
    void refreshDeviceFleet({ silent: true });
  } catch (error) {
    const message = errorMessage(error);
    actionFeedback.set(key, { action, tone: "error", message });
    bindings?.setStatus?.(`${device.name} · ${message}`);
  } finally {
    inFlightActions.delete(key);
    requestRerender();
  }
};

const manualActionDeviceKey = (
  request: Readonly<ManualUsbDeviceActionRequest>,
): string | null => {
  const requestedKey = request.deviceKey?.trim();
  const requestedId = request.deviceId?.trim();
  if (requestedKey) {
    const device = deviceByKey(requestedKey);
    if (!device || (requestedId && device.id !== requestedId)) return null;
    return device.key;
  }
  if (!requestedId) return null;
  const matches = snapshot?.devices.filter((device) => device.id === requestedId) ?? [];
  return matches.length === 1 ? matches[0].key : null;
};

const handleManualUsbDeviceAction = (
  request: Readonly<ManualUsbDeviceActionRequest>,
): void => {
  const key = manualActionDeviceKey(request);
  if (!key) {
    throw new Error("L’appareil demandé n’est plus disponible ou ne correspond plus à la sélection.");
  }
  if (!DEVICE_ACTION_SET.has(request.action)) {
    throw new Error("Cette action appareil n’est pas reconnue.");
  }
  const device = deviceByKey(key);
  if (!device) {
    throw new Error("L’appareil demandé n’est plus disponible.");
  }
  if (!device.ready && request.action !== "info") {
    throw new Error("Cet appareil n’est plus prêt pour recevoir une action.");
  }
  if (inFlightActions.has(key)) {
    throw new Error("Une action est déjà en cours sur cet appareil.");
  }
  if (!supportsAction(device, request.action)) {
    throw new Error("Cette action n’est pas annoncée par le contrôleur de cet appareil.");
  }
  const readOnly = request.action === "info" || request.action === "screenshot";
  if (!readOnly && request.confirmed !== true) {
    setValidationError(key, request.action, "Cette action exige une confirmation humaine explicite.");
    return;
  }
  if (request.action === "shell") {
    const command = typeof request.args?.command === "string" ? request.args.command : "";
    if (!command || request.exactConfirmation !== command) {
      setValidationError(key, request.action, "La commande shell exacte doit être confirmée.");
      return;
    }
  }
  void performDeviceAction(key, request.action, request.args, request.confirmed === true);
};

const platformLabel = (platform: DevicePlatform): string => ({
  android: "Android",
  ios: "iOS",
  unknown: "Mobile",
})[platform];

const platformIcon = (platform: DevicePlatform): string =>
  platform === "android" ? "smartphone" : platform === "ios" ? "app-window" : "usb";

const actionLabel = (action: DeviceControlAction): string => ({
  info: "Informations",
  screenshot: "Capture",
  open_screen: "Afficher l’écran",
  tap: "Appui",
  swipe: "Balayage",
  type_text: "Saisir du texte",
  key_event: "Touche système",
  open_app: "Ouvrir une app",
  shell: "Shell",
  push_file: "Pousser un fichier",
})[action];

const actionIcon = (action: DeviceControlAction): string => ({
  info: "info",
  screenshot: "scan-line",
  open_screen: "eye",
  tap: "mouse-pointer-2",
  swipe: "route",
  type_text: "type",
  key_event: "keyboard",
  open_app: "app-window",
  shell: "square-terminal",
  push_file: "upload",
})[action];

const formatUpdatedAt = (value: number | null): string => {
  if (value === null) return "Mise à jour automatique toutes les 4 secondes";
  try {
    return `Dernière détection ${new Intl.DateTimeFormat("fr-FR", {
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    }).format(new Date(value))}`;
  } catch {
    return "Détection actualisée";
  }
};

const renderFeedback = (key: string): string => {
  const feedback = actionFeedback.get(key);
  if (!feedback) return "";
  const resultText = feedback.action === "info" || feedback.action === "shell"
    ? displayResult(feedback.result)
    : "";
  return `<div class="device-fleet-feedback is-${feedback.tone}" role="${feedback.tone === "error" ? "alert" : "status"}">
    <span><i data-lucide="${feedback.tone === "success" ? "badge-check" : feedback.tone === "warning" ? "triangle-alert" : "circle-alert"}"></i>${escapeHtml(feedback.message)}</span>
    ${resultText ? `<pre>${escapeHtml(resultText)}</pre>` : ""}
  </div>`;
};

const renderMedia = (device: DeviceFleetDevice): string => {
  const media = deviceMedia.get(device.key);
  if (!media) return "";
  const label = media.kind === "screen" ? `Écran de ${device.name}` : `Capture de ${device.name}`;
  return `<section class="device-fleet-media is-${media.kind}">
    <header><span><i data-lucide="${media.kind === "screen" ? "eye" : "scan-line"}"></i><strong>${escapeHtml(label)}</strong></span><button type="button" class="icon-button" data-device-close-media="${escapeAttr(device.key)}" aria-label="Fermer ${escapeAttr(label)}"><i data-lucide="circle-x"></i></button></header>
    ${media.kind === "screen"
      ? `<iframe src="${escapeAttr(media.url)}" title="${escapeAttr(label)}" sandbox="allow-scripts allow-same-origin" allow="fullscreen" referrerpolicy="no-referrer"></iframe>`
      : `<img src="${escapeAttr(media.url)}" alt="${escapeAttr(label)}" referrerpolicy="no-referrer" />`}
  </section>`;
};

const renderActionButton = (
  device: DeviceFleetDevice,
  action: "info" | "screenshot" | "open_screen",
): string => {
  const running = inFlightActions.get(device.key);
  const supported = supportsAction(device, action);
  const disabled = Boolean(running) || (!device.ready && action !== "info") || !supported;
  return `<button type="button" class="tool-button ${action === "open_screen" ? "primary" : ""}" data-device-action="${action}" data-device-key="${escapeAttr(device.key)}" ${disabled ? "disabled" : ""} title="${supported ? escapeAttr(actionLabel(action)) : "Action non prise en charge"}">
    <i data-lucide="${running === action ? "loader-circle" : actionIcon(action)}" class="${running === action ? "is-spinning" : ""}"></i><span>${escapeHtml(running === action ? "En cours…" : actionLabel(action))}</span>
  </button>`;
};

const formDisabled = (device: DeviceFleetDevice, action: DeviceControlAction): boolean =>
  Boolean(inFlightActions.get(device.key)) || !device.ready || !supportsAction(device, action);

const renderDeviceControls = (device: DeviceFleetDevice): string => {
  const draft = draftFor(device.key);
  const pendingShell = pendingShellConfirmations.get(device.key);
  const pendingPush = pendingPushFileConfirmations.get(device.key);
  const open = expandedControls.has(device.key);
  const fieldAttrs = `data-device-key="${escapeAttr(device.key)}" data-device-draft`;
  return `<details class="device-fleet-controls" data-device-controls="${escapeAttr(device.key)}" ${open ? "open" : ""}>
    <summary><span><i data-lucide="mouse-pointer-2"></i><strong>Contrôles avancés</strong></span><small>Appui, balayage, texte, application et shell</small><i data-lucide="chevron-down"></i></summary>
    <div class="device-fleet-control-grid">
      <form data-device-action-form="tap" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="mouse-pointer-2"></i><strong>Appui</strong></header>
        <div class="device-fleet-coordinate-row">
          <label><span>X</span><input ${fieldAttrs} data-device-draft-field="tapX" name="x" type="number" min="0" step="1" inputmode="numeric" value="${escapeAttr(draft.tapX)}" required /></label>
          <label><span>Y</span><input ${fieldAttrs} data-device-draft-field="tapY" name="y" type="number" min="0" step="1" inputmode="numeric" value="${escapeAttr(draft.tapY)}" required /></label>
        </div>
        <button type="submit" class="tool-button" ${formDisabled(device, "tap") ? "disabled" : ""}><i data-lucide="target"></i><span>Envoyer l’appui</span></button>
      </form>

      <form data-device-action-form="swipe" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="route"></i><strong>Balayage</strong></header>
        <div class="device-fleet-coordinate-row four">
          <label><span>Départ X</span><input ${fieldAttrs} data-device-draft-field="swipeStartX" name="startX" type="number" min="0" step="1" value="${escapeAttr(draft.swipeStartX)}" required /></label>
          <label><span>Départ Y</span><input ${fieldAttrs} data-device-draft-field="swipeStartY" name="startY" type="number" min="0" step="1" value="${escapeAttr(draft.swipeStartY)}" required /></label>
          <label><span>Arrivée X</span><input ${fieldAttrs} data-device-draft-field="swipeEndX" name="endX" type="number" min="0" step="1" value="${escapeAttr(draft.swipeEndX)}" required /></label>
          <label><span>Arrivée Y</span><input ${fieldAttrs} data-device-draft-field="swipeEndY" name="endY" type="number" min="0" step="1" value="${escapeAttr(draft.swipeEndY)}" required /></label>
        </div>
        <label><span>Durée (ms)</span><input ${fieldAttrs} data-device-draft-field="swipeDurationMs" name="durationMs" type="number" min="50" max="10000" step="10" value="${escapeAttr(draft.swipeDurationMs)}" required /></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "swipe") ? "disabled" : ""}><i data-lucide="route"></i><span>Balayer</span></button>
      </form>

      <form data-device-action-form="type_text" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="type"></i><strong>Saisir du texte</strong></header>
        <label><span>Texte exact</span><textarea ${fieldAttrs} data-device-draft-field="text" name="text" maxlength="${MAX_TEXT_LENGTH}" rows="3" required>${escapeHtml(draft.text)}</textarea></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "type_text") ? "disabled" : ""}><i data-lucide="send"></i><span>Envoyer le texte</span></button>
      </form>

      <form data-device-action-form="key_event" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="keyboard"></i><strong>Touche système</strong></header>
        <label><span>Touche</span><select ${fieldAttrs} data-device-draft-field="key" name="key">
          ${["HOME", "BACK", "ENTER", "POWER", "APP_SWITCH", "VOLUME_UP", "VOLUME_DOWN"].map((key) => `<option value="${key}" ${draft.key === key ? "selected" : ""}>${key.replaceAll("_", " ")}</option>`).join("")}
        </select></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "key_event") ? "disabled" : ""}><i data-lucide="keyboard"></i><span>Envoyer la touche</span></button>
      </form>

      <form data-device-action-form="open_app" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="app-window"></i><strong>Ouvrir une application</strong></header>
        <label><span>Package Android ou bundle iOS</span><input ${fieldAttrs} data-device-draft-field="appId" name="appId" maxlength="255" autocomplete="off" spellcheck="false" placeholder="com.exemple.application" value="${escapeAttr(draft.appId)}" required /></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "open_app") ? "disabled" : ""}><i data-lucide="play"></i><span>Ouvrir l’application</span></button>
      </form>

      <form class="device-fleet-shell-form" data-device-action-form="shell" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="square-terminal"></i><strong>Commande shell</strong><span><i data-lucide="lock-keyhole"></i>Confirmation obligatoire</span></header>
        <label><span>Commande exacte</span><textarea ${fieldAttrs} data-device-draft-field="shell" name="command" maxlength="${MAX_COMMAND_LENGTH}" rows="3" autocomplete="off" spellcheck="false" placeholder="Commande exécutée sur cet appareil" required>${escapeHtml(draft.shell)}</textarea></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "shell") ? "disabled" : ""}><i data-lucide="shield-question"></i><span>Vérifier avant exécution</span></button>
        ${pendingShell ? `<div class="device-fleet-shell-confirm" role="alertdialog" aria-label="Confirmer la commande shell">
          <span><i data-lucide="triangle-alert"></i><strong>Confirmer sur ${escapeHtml(device.name)}</strong><small>Cette commande peut modifier l’appareil.</small></span>
          <code>${escapeHtml(pendingShell)}</code>
          <div><button type="button" class="tool-button" data-device-shell-cancel="${escapeAttr(device.key)}"><i data-lucide="circle-x"></i><span>Annuler</span></button><button type="button" class="tool-button danger" data-device-shell-confirm="${escapeAttr(device.key)}"><i data-lucide="square-terminal"></i><span>Confirmer l’exécution</span></button></div>
        </div>` : ""}
      </form>

      <form class="device-fleet-shell-form" data-device-action-form="push_file" data-device-key="${escapeAttr(device.key)}">
        <header><i data-lucide="upload"></i><strong>Pousser un fichier (PC → appareil)</strong><span><i data-lucide="lock-keyhole"></i>Confirmation obligatoire</span></header>
        <label><span>Fichier local (ce poste)</span><input ${fieldAttrs} data-device-draft-field="localPath" name="localPath" type="text" maxlength="${MAX_PATH_LENGTH}" autocomplete="off" spellcheck="false" placeholder="C:\\Videos\\ma-video.mp4" value="${escapeAttr(draft.localPath)}" required /></label>
        <label><span>Destination sur l’appareil</span><input ${fieldAttrs} data-device-draft-field="remotePath" name="remotePath" type="text" maxlength="${MAX_PATH_LENGTH}" autocomplete="off" spellcheck="false" placeholder="/sdcard/Download/ma-video.mp4" value="${escapeAttr(draft.remotePath)}" required /></label>
        <button type="submit" class="tool-button" ${formDisabled(device, "push_file") ? "disabled" : ""}><i data-lucide="shield-question"></i><span>Vérifier avant exécution</span></button>
        ${pendingPush ? `<div class="device-fleet-shell-confirm" role="alertdialog" aria-label="Confirmer le transfert de fichier">
          <span><i data-lucide="triangle-alert"></i><strong>Confirmer sur ${escapeHtml(device.name)}</strong><small>Le fichier sera copié depuis ce poste vers l’appareil.</small></span>
          <code>adb push ${escapeHtml(pendingPush.localPath)} ${escapeHtml(pendingPush.remotePath)}</code>
          <div><button type="button" class="tool-button" data-device-push-cancel="${escapeAttr(device.key)}"><i data-lucide="circle-x"></i><span>Annuler</span></button><button type="button" class="tool-button danger" data-device-push-confirm="${escapeAttr(device.key)}"><i data-lucide="upload"></i><span>Confirmer le transfert</span></button></div>
        </div>` : ""}
      </form>
    </div>
  </details>`;
};

const renderDeviceCard = (device: DeviceFleetDevice): string => {
  const running = inFlightActions.get(device.key);
  const capabilityLabel = device.capabilities === null
    ? "Capacités à la demande"
    : `${device.capabilities.length} action${device.capabilities.length === 1 ? "" : "s"}`;
  return `<article class="device-fleet-card platform-${device.platform} state-${device.state}" data-device-card="${escapeAttr(device.key)}">
    <header class="device-fleet-card-head">
      <span class="device-fleet-platform-mark"><i data-lucide="${platformIcon(device.platform)}"></i></span>
      <span class="device-fleet-card-copy"><small>${escapeHtml(platformLabel(device.platform))} · ${escapeHtml(device.transport)}</small><strong>${escapeHtml(device.name)}</strong><code>${escapeHtml(device.serial)}</code></span>
      <span class="device-fleet-state is-${device.state}"><i data-lucide="${device.state === "ready" ? "badge-check" : device.state === "unauthorized" ? "lock" : device.state === "busy" ? "users" : device.state === "error" ? "circle-alert" : "wifi-off"}"></i>${escapeHtml(device.stateLabel)}</span>
    </header>
    <div class="device-fleet-meta">
      ${device.model ? `<span><i data-lucide="smartphone"></i>${escapeHtml(device.model)}</span>` : ""}
      ${device.osVersion ? `<span><i data-lucide="cpu"></i>${escapeHtml(device.osVersion)}</span>` : ""}
      ${device.batteryLevel !== null ? `<span><i data-lucide="gauge"></i>${device.batteryLevel}%</span>` : ""}
      <span><i data-lucide="wrench"></i>${escapeHtml(capabilityLabel)}</span>
      ${device.owner ? `<span class="is-owner"><i data-lucide="users"></i>${escapeHtml(device.owner)}</span>` : ""}
    </div>
    ${running ? `<div class="device-fleet-running" role="status"><i data-lucide="loader-circle" class="is-spinning"></i><span>${escapeHtml(actionLabel(running))} en cours…</span></div>` : ""}
    <div class="device-fleet-primary-actions">
      ${renderActionButton(device, "info")}
      ${renderActionButton(device, "screenshot")}
      ${renderActionButton(device, "open_screen")}
    </div>
    ${renderMedia(device)}
    ${renderFeedback(device.key)}
    ${renderDeviceControls(device)}
    <section class="device-fleet-extension-slot" data-usb-device-extension="assisted-navigation" data-device-key="${escapeAttr(device.key)}" data-device-id="${escapeAttr(device.id)}" aria-label="Navigation assistée"></section>
  </article>`;
};

const filteredDevices = (): DeviceFleetDevice[] => {
  const devices = snapshot?.devices ?? [];
  if (activeFilter === "all") return devices;
  if (activeFilter === "ready") return devices.filter((device) => device.ready);
  return devices.filter((device) => device.platform === activeFilter);
};

const renderFilters = (devices: readonly DeviceFleetDevice[]): string => {
  const counts = {
    all: devices.length,
    android: devices.filter((device) => device.platform === "android").length,
    ios: devices.filter((device) => device.platform === "ios").length,
    ready: devices.filter((device) => device.ready).length,
  };
  const labels: Record<DeviceFleetFilter, string> = {
    all: "Tous",
    android: "Android",
    ios: "iOS",
    ready: "Prêts",
  };
  return `<div class="device-fleet-filters" role="group" aria-label="Filtrer les appareils">
    ${(Object.keys(labels) as DeviceFleetFilter[]).map((filter) => `<button type="button" data-device-filter="${filter}" class="${activeFilter === filter ? "active" : ""}" aria-pressed="${activeFilter === filter}"><span>${labels[filter]}</span><b>${counts[filter]}</b></button>`).join("")}
  </div>`;
};

const renderToolStatus = (tools: readonly DeviceFleetTool[]): string => {
  if (!tools.length) return "";
  return `<section class="device-fleet-tools" aria-label="Disponibilité des outils USB">
    <span><i data-lucide="wrench"></i>Outils du connecteur</span>
    <div>${tools.map((tool) => `<span class="is-${tool.available === true ? "available" : tool.available === false ? "missing" : "unknown"}" ${tool.detail ? `title="${escapeAttr(tool.detail)}"` : ""}><i data-lucide="${tool.available === true ? "badge-check" : tool.available === false ? "circle-x" : "info"}"></i>${escapeHtml(tool.label)}</span>`).join("")}</div>
  </section>`;
};

export const renderDeviceFleetPanel = ({ remoteMode }: DeviceFleetRenderOptions): string => {
  if (!remoteMode) {
    return `<div id="deviceFleetPanel" class="device-fleet-panel"><section class="device-fleet-unavailable"><span><i data-lucide="server"></i></span><h2>Connexion Switch requise</h2><p>La flotte USB est pilotée par le service de Switch développement. Ouvrez cette page depuis l’interface connectée au serveur.</p></section></div>`;
  }
  const devices = snapshot?.devices ?? [];
  const visible = filteredDevices();
  const readyCount = devices.filter((device) => device.ready).length;
  const bridgeOnline = snapshot?.bridgeOnline;
  return `<div id="deviceFleetPanel" class="device-fleet-panel">
    <section class="device-fleet-hero">
      <div class="device-fleet-hero-copy"><span><i data-lucide="usb"></i></span><div><small>USB · Android et iOS jailbreak</small><h2>Flotte d’appareils</h2><p>Contrôlez plusieurs appareils indépendamment depuis Switch, sans mélanger leurs actions ni leurs résultats.</p></div></div>
      <div class="device-fleet-summary">
        <span class="device-fleet-bridge ${bridgeOnline === false ? "is-offline" : bridgeOnline === true ? "is-online" : "is-unknown"}"><i data-lucide="${bridgeOnline === false ? "unplug" : "plug-zap"}"></i>${bridgeOnline === false ? "Pont hors ligne" : bridgeOnline === true ? "Pont en ligne" : "Pont en détection"}</span>
        <span><strong>${readyCount}</strong><small>prêt${readyCount === 1 ? "" : "s"}</small></span><span><strong>${devices.length}</strong><small>détecté${devices.length === 1 ? "" : "s"}</small></span>
        <button type="button" id="deviceFleetRefresh" class="icon-button" aria-label="Actualiser les appareils" title="Actualiser" ${loading ? "disabled" : ""}><i data-lucide="refresh-cw" class="${loading ? "is-spinning" : ""}"></i></button>
      </div>
    </section>
    <section class="device-fleet-toolbar"><span><i data-lucide="radio"></i>${escapeHtml(formatUpdatedAt(snapshot?.updatedAt ?? null))}</span>${renderFilters(devices)}</section>
    ${renderToolStatus(snapshot?.tools ?? [])}
    ${loadError ? `<div class="device-fleet-page-feedback is-error" role="alert"><i data-lucide="circle-alert"></i><span><strong>Détection impossible</strong><small>${escapeHtml(loadError)}</small></span><button type="button" class="tool-button" data-device-retry><i data-lucide="refresh-cw"></i><span>Réessayer</span></button></div>` : ""}
    ${snapshot?.warning ? `<div class="device-fleet-page-feedback is-warning" role="status"><i data-lucide="triangle-alert"></i><span>${escapeHtml(snapshot.warning)}</span></div>` : ""}
    ${loading && !snapshot ? `<section class="device-fleet-loading" role="status"><i data-lucide="loader-circle" class="is-spinning"></i><strong>Détection des appareils USB…</strong><small>Android ADB et iOS jailbreak</small></section>` : ""}
    ${snapshot && !devices.length ? `<section class="device-fleet-empty"><span><i data-lucide="usb"></i></span><h3>Aucun appareil détecté</h3><p>Branchez un appareil, autorisez la connexion USB puis actualisez la flotte.</p></section>` : ""}
    ${devices.length && !visible.length ? `<section class="device-fleet-empty compact"><span><i data-lucide="list-filter"></i></span><h3>Aucun appareil dans ce filtre</h3><button type="button" class="tool-button" data-device-filter="all"><i data-lucide="refresh-ccw"></i><span>Afficher toute la flotte</span></button></section>` : ""}
    ${visible.length ? `<section class="device-fleet-grid" aria-label="Appareils contrôlables">${visible.map(renderDeviceCard).join("")}</section>` : ""}
  </div>`;
};

const readInteger = (
  form: HTMLFormElement,
  name: string,
  label: string,
  min: number,
  max = Number.MAX_SAFE_INTEGER,
): number => {
  const input = form.elements.namedItem(name) as HTMLInputElement | null;
  const value = Number(input?.value);
  if (!Number.isInteger(value) || value < min || value > max) {
    throw new Error(`${label} doit être un entier entre ${min} et ${max}.`);
  }
  return value;
};

const rememberDraftField = (target: HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement) => {
  const key = target.dataset.deviceKey;
  const field = target.dataset.deviceDraftField as keyof DeviceDraft | undefined;
  if (!key || !field || !(field in DEFAULT_DRAFT)) return;
  const draft = draftFor(key);
  draft[field] = target.value;
};

const formActionArgs = (
  form: HTMLFormElement,
  action: DeviceControlAction,
): Record<string, unknown> => {
  if (action === "tap") {
    return {
      x: readInteger(form, "x", "X", 0),
      y: readInteger(form, "y", "Y", 0),
    };
  }
  if (action === "swipe") {
    return {
      startX: readInteger(form, "startX", "Départ X", 0),
      startY: readInteger(form, "startY", "Départ Y", 0),
      endX: readInteger(form, "endX", "Arrivée X", 0),
      endY: readInteger(form, "endY", "Arrivée Y", 0),
      durationMs: readInteger(form, "durationMs", "Durée", 50, 10_000),
    };
  }
  if (action === "type_text") {
    const text = (form.elements.namedItem("text") as HTMLTextAreaElement | null)?.value ?? "";
    if (!text.trim()) throw new Error("Le texte à saisir est vide.");
    if (text.length > MAX_TEXT_LENGTH) throw new Error("Le texte est trop long.");
    return { text };
  }
  if (action === "key_event") {
    const key = (form.elements.namedItem("key") as HTMLSelectElement | null)?.value.trim() ?? "";
    if (!key) throw new Error("Choisissez une touche système.");
    return { key };
  }
  if (action === "open_app") {
    const appId = (form.elements.namedItem("appId") as HTMLInputElement | null)?.value.trim() ?? "";
    if (!appId) throw new Error("Le package ou bundle de l’application est requis.");
    return { appId };
  }
  if (action === "shell") {
    const command = (form.elements.namedItem("command") as HTMLTextAreaElement | null)?.value.trim() ?? "";
    if (!command) throw new Error("La commande shell est vide.");
    if (command.length > MAX_COMMAND_LENGTH) throw new Error("La commande shell est trop longue.");
    return { command };
  }
  if (action === "push_file") {
    const localPath = (form.elements.namedItem("localPath") as HTMLInputElement | null)?.value.trim() ?? "";
    const remotePath = (form.elements.namedItem("remotePath") as HTMLInputElement | null)?.value.trim() ?? "";
    if (!localPath) throw new Error("Le chemin du fichier local est vide.");
    if (!remotePath) throw new Error("La destination sur l’appareil est vide.");
    if (localPath.length > MAX_PATH_LENGTH || remotePath.length > MAX_PATH_LENGTH) {
      throw new Error("Un chemin est trop long.");
    }
    return { localPath, remotePath };
  }
  return {};
};

const setValidationError = (key: string, action: DeviceControlAction, error: unknown) => {
  actionFeedback.set(key, { action, tone: "error", message: errorMessage(error) });
  requestRerender();
};

export const bindDeviceFleetUi = (nextBindings: DeviceFleetBindings): void => {
  const root = document.querySelector<HTMLElement>("#deviceFleetPanel");
  if (!root) return;
  bindings = nextBindings;
  root.querySelector<HTMLButtonElement>("#deviceFleetRefresh")?.addEventListener("click", () => {
    void refreshDeviceFleet();
  });
  root.querySelector<HTMLButtonElement>("[data-device-retry]")?.addEventListener("click", () => {
    void refreshDeviceFleet();
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-filter]").forEach((button) => {
    button.addEventListener("click", () => {
      const filter = button.dataset.deviceFilter;
      if (filter !== "all" && filter !== "android" && filter !== "ios" && filter !== "ready") return;
      activeFilter = filter;
      nextBindings.rerender();
    });
  });
  root.querySelectorAll<HTMLInputElement | HTMLTextAreaElement | HTMLSelectElement>("[data-device-draft]").forEach((field) => {
    field.addEventListener("input", () => rememberDraftField(field));
    field.addEventListener("change", () => rememberDraftField(field));
  });
  root.querySelectorAll<HTMLDetailsElement>("[data-device-controls]").forEach((details) => {
    details.addEventListener("toggle", () => {
      const key = details.dataset.deviceControls;
      if (!key) return;
      if (details.open) expandedControls.add(key);
      else expandedControls.delete(key);
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-action]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.deviceKey;
      const action = button.dataset.deviceAction;
      if (!key || !action || !DEVICE_ACTION_SET.has(action)) return;
      void performDeviceAction(
        key,
        action as DeviceControlAction,
        undefined,
        action === "open_screen",
      );
    });
  });
  root.querySelectorAll<HTMLFormElement>("[data-device-action-form]").forEach((form) => {
    form.addEventListener("submit", (event) => {
      event.preventDefault();
      const key = form.dataset.deviceKey;
      const action = form.dataset.deviceActionForm;
      if (!key || !action || !DEVICE_ACTION_SET.has(action)) return;
      try {
        const args = formActionArgs(form, action as DeviceControlAction);
        if (action === "shell") {
          pendingShellConfirmations.set(key, String(args.command));
          actionFeedback.delete(key);
          nextBindings.rerender();
          return;
        }
        if (action === "push_file") {
          const localPath = String(args.localPath ?? "");
          const remotePath = String(args.remotePath ?? "");
          if (!localPath || !remotePath) return;
          pendingPushFileConfirmations.set(key, { localPath, remotePath });
          actionFeedback.delete(key);
          nextBindings.rerender();
          return;
        }
        void performDeviceAction(key, action as DeviceControlAction, args, true);
      } catch (error) {
        setValidationError(key, action as DeviceControlAction, error);
      }
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-shell-cancel]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.deviceShellCancel;
      if (!key) return;
      pendingShellConfirmations.delete(key);
      nextBindings.rerender();
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-shell-confirm]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.deviceShellConfirm;
      const command = key ? pendingShellConfirmations.get(key) : null;
      if (!key || !command) return;
      pendingShellConfirmations.delete(key);
      void performDeviceAction(key, "shell", { command }, true);
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-push-cancel]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.devicePushCancel;
      if (!key) return;
      pendingPushFileConfirmations.delete(key);
      nextBindings.rerender();
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-push-confirm]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.devicePushConfirm;
      const pending = key ? pendingPushFileConfirmations.get(key) : null;
      if (!key || !pending) return;
      pendingPushFileConfirmations.delete(key);
      void performDeviceAction(
        key,
        "push_file",
        { localPath: pending.localPath, remotePath: pending.remotePath },
        true,
      );
    });
  });
  root.querySelectorAll<HTMLButtonElement>("[data-device-close-media]").forEach((button) => {
    button.addEventListener("click", () => {
      const key = button.dataset.deviceCloseMedia;
      if (!key) return;
      deviceMedia.delete(key);
      nextBindings.rerender();
    });
  });
  window.dispatchEvent(new CustomEvent(USB_DEVICE_PANEL_READY_EVENT, {
    detail: {
      rootId: root.id,
      slotSelector: USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR,
      deviceKeys: (snapshot?.devices ?? []).map((device) => device.key),
    },
  }));
};

export const activateDeviceFleetPanel = (
  rerender: () => void,
  shouldPoll = true,
): void => {
  active = true;
  bindings = { ...(bindings ?? {}), rerender };
  if (!releaseAssistedActionHandler) {
    releaseAssistedActionHandler = registerManualUsbDeviceActionHandler(
      handleManualUsbDeviceAction,
    );
  }
  if (!shouldPoll) return;
  if (pollTimer === null) {
    pollTimer = window.setInterval(() => {
      if (!active || document.visibilityState === "hidden") return;
      void refreshDeviceFleet({ silent: true });
    }, DEVICE_FLEET_POLL_MS);
  }
  void refreshDeviceFleet({ silent: snapshot !== null });
};

export const deactivateDeviceFleetPanel = (): void => {
  active = false;
  bindings = null;
  releaseAssistedActionHandler?.();
  releaseAssistedActionHandler = null;
  pendingShellConfirmations.clear();
  deviceMedia.clear();
  if (pollTimer !== null) {
    window.clearInterval(pollTimer);
    pollTimer = null;
  }
};
