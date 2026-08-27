import type { DeviceControlAction } from "./device-fleet";

export const USB_DEVICE_PANEL_READY_EVENT = "switch:usb-device-panel-ready";
export const USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR =
  '[data-usb-device-extension="assisted-navigation"]';

export type ManualUsbDeviceActionRequest = {
  deviceKey?: string;
  deviceId?: string;
  action: DeviceControlAction;
  args?: Record<string, unknown>;
  confirmed?: boolean;
  exactConfirmation?: string;
  source: string;
};

export type ManualUsbDeviceActionHandler = (
  request: Readonly<ManualUsbDeviceActionRequest>,
) => void;

const READ_ONLY_ACTIONS = new Set<DeviceControlAction>(["info", "screenshot"]);
const TRUSTED_GESTURE_EVENTS = new Set(["click", "submit"]);
const consumedGestureEvents = new WeakSet<Event>();
let manualUsbDeviceActionHandler: ManualUsbDeviceActionHandler | null = null;

/**
 * Connects the device-fleet controller to optional assistants without exposing
 * a privileged DOM event that another page script could forge.
 */
export const registerManualUsbDeviceActionHandler = (
  handler: ManualUsbDeviceActionHandler,
): (() => void) => {
  if (manualUsbDeviceActionHandler && manualUsbDeviceActionHandler !== handler) {
    throw new Error("Un contrôleur d’actions appareil est déjà enregistré.");
  }
  manualUsbDeviceActionHandler = handler;
  let registered = true;
  return () => {
    if (registered && manualUsbDeviceActionHandler === handler) {
      manualUsbDeviceActionHandler = null;
    }
    registered = false;
  };
};

/**
 * Stable integration contract for optional USB-device assistants.
 *
 * The caller must invoke this synchronously from a real click/submit handler.
 * Mutating actions require an explicit confirmation rendered by the caller;
 * shell additionally requires the exact command to be repeated unchanged.
 */
export const requestManualUsbDeviceAction = (
  trigger: Event,
  request: ManualUsbDeviceActionRequest,
): void => {
  if (
    typeof Event === "undefined"
    || !(trigger instanceof Event)
    || !trigger.isTrusted
    || !TRUSTED_GESTURE_EVENTS.has(trigger.type)
  ) {
    throw new Error("Une action appareil assistée doit partir d’un clic humain explicite.");
  }
  if (!navigator.userActivation?.isActive) {
    throw new Error("Le geste utilisateur n’est plus actif.");
  }
  if (consumedGestureEvents.has(trigger)) {
    throw new Error("Ce geste utilisateur a déjà autorisé une action appareil.");
  }
  consumedGestureEvents.add(trigger);
  if (!request.deviceKey?.trim() && !request.deviceId?.trim()) {
    throw new Error("Un appareil cible explicite est requis.");
  }
  if (!request.source?.trim() || request.source.length > 80) {
    throw new Error("La source de l’action assistée est invalide.");
  }
  if (!READ_ONLY_ACTIONS.has(request.action) && request.confirmed !== true) {
    throw new Error("Cette action appareil exige une confirmation humaine explicite.");
  }
  if (request.action === "shell") {
    const command = typeof request.args?.command === "string" ? request.args.command : "";
    if (!command || request.exactConfirmation !== command) {
      throw new Error("La commande shell exacte doit être confirmée sans modification.");
    }
  }
  if (!manualUsbDeviceActionHandler) {
    throw new Error("Le contrôleur de la page Appareils n’est pas actif.");
  }
  manualUsbDeviceActionHandler(Object.freeze({
    ...request,
    deviceKey: request.deviceKey?.trim() || undefined,
    deviceId: request.deviceId?.trim() || undefined,
    source: request.source.trim(),
  }));
};

if (typeof window !== "undefined" && typeof document !== "undefined") {
  void import("./tiktok-assisted-navigation")
    .then(({ installTikTokAssistedNavigation }) => {
      installTikTokAssistedNavigation({
        requestManualAction: requestManualUsbDeviceAction,
        panelReadyEvent: USB_DEVICE_PANEL_READY_EVENT,
        slotSelector: USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR,
      });
    })
    .catch((error) => {
      console.error("Navigation assistée TikTok indisponible", error);
    });
}
