import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const usb = read("../src/usb-devices.ts");
const terminal = read("../src/terminal-runtime.ts");
const allocation = read("../src/remote-allocation.ts");
const main = read("../src/main.ts");

const block = (source, start, end) => {
  const from = source.indexOf(start);
  const to = source.indexOf(end, from + start.length);
  assert.ok(from >= 0, `debut introuvable: ${start}`);
  assert.ok(to > from, `fin introuvable: ${end}`);
  return source.slice(from, to);
};

test("le contrat USB expose ses constantes et types", () => {
  assert.match(usb, /export const USB_DEVICE_PANEL_READY_EVENT = "switch:usb-device-panel-ready";/);
  assert.match(usb, /export const USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR =/);
  assert.match(usb, /\[data-usb-device-extension="assisted-navigation"\]/);
  assert.match(usb, /export type ManualUsbDeviceActionRequest = \{/);
  assert.match(usb, /deviceKey\?: string;/);
  assert.match(usb, /deviceId\?: string;/);
  assert.match(usb, /action: DeviceControlAction;/);
  assert.match(usb, /args\?: Record<string, unknown>;/);
  assert.match(usb, /confirmed\?: boolean;/);
  assert.match(usb, /exactConfirmation\?: string;/);
  assert.match(usb, /source: string;/);
  assert.match(usb, /export type ManualUsbDeviceActionHandler = \(/);
  assert.match(usb, /request: Readonly<ManualUsbDeviceActionRequest>,/);
});

test("les actions en lecture seule sont bornées à info et screenshot", () => {
  const readOnly = block(usb, "const READ_ONLY_ACTIONS", "const consumedGestureEvents");
  assert.match(readOnly, /new Set<DeviceControlAction>\(\["info", "screenshot"\]\)/);
  assert.match(readOnly, /"info"/);
  assert.match(readOnly, /"screenshot"/);
  assert.match(usb, /const TRUSTED_GESTURE_EVENTS = new Set\(\["click", "submit"\]\);/);
  assert.match(usb, /const consumedGestureEvents = new WeakSet<Event>\(\);/);
  assert.ok(readOnly.indexOf("info") >= 0, "info en lecture seule");
  assert.ok(readOnly.indexOf("screenshot") >= 0, "screenshot en lecture seule");
});

test("un seul contrôleur d'actions peut être enregistré à la fois", () => {
  const register = block(usb, "export const registerManualUsbDeviceActionHandler", "export const requestManualUsbDeviceAction");
  assert.match(register, /if \(manualUsbDeviceActionHandler && manualUsbDeviceActionHandler !== handler\) \{/);
  assert.match(register, /Un contrôleur d’actions appareil est déjà enregistré\./);
  assert.match(register, /manualUsbDeviceActionHandler = handler;/);
  assert.match(register, /let registered = true;/);
  assert.match(register, /return \(\) => \{/);
  assert.match(register, /if \(registered && manualUsbDeviceActionHandler === handler\) \{/);
  assert.match(register, /manualUsbDeviceActionHandler = null;/);
  assert.match(register, /registered = false;/);
  assert.ok(register.indexOf("throw new Error") >= 0, "refus du double contrôleur");
});

test("une action assistée exige un geste humain explicite et non consommé", () => {
  const request = block(usb, "export const requestManualUsbDeviceAction", "if (typeof window");
  assert.match(request, /typeof Event === "undefined"/);
  assert.match(request, /trigger instanceof Event/);
  assert.match(request, /trigger\.isTrusted/);
  assert.match(request, /TRUSTED_GESTURE_EVENTS\.has\(trigger\.type\)/);
  assert.match(request, /Une action appareil assistée doit partir d’un clic humain explicite\./);
  assert.match(request, /navigator\.userActivation\?\.isActive/);
  assert.match(request, /Le geste utilisateur n’est plus actif\./);
  assert.match(request, /consumedGestureEvents\.has\(trigger\)/);
  assert.match(request, /Ce geste utilisateur a déjà autorisé une action appareil\./);
  assert.match(request, /consumedGestureEvents\.add\(trigger\);/);
  assert.ok(request.indexOf("isTrusted") >= 0, "événement digne de confiance");
  assert.ok(request.indexOf("userActivation") >= 0, "activation utilisateur");
});

test("une action assistée exige une cible et une source valides", () => {
  const request = block(usb, "export const requestManualUsbDeviceAction", "if (typeof window");
  assert.match(request, /!request\.deviceKey\?\.trim\(\) && !request\.deviceId\?\.trim\(\)/);
  assert.match(request, /Un appareil cible explicite est requis\./);
  assert.match(request, /!request\.source\?\.trim\(\) \|\| request\.source\.length > 80/);
  assert.match(request, /La source de l’action assistée est invalide\./);
  assert.ok(request.indexOf("deviceKey") >= 0, "clé d'appareil");
  assert.ok(request.indexOf("deviceId") >= 0, "id d'appareil");
});

test("toute action mutante exige une confirmation humaine explicite", () => {
  const request = block(usb, "export const requestManualUsbDeviceAction", "if (typeof window");
  assert.match(request, /!READ_ONLY_ACTIONS\.has\(request\.action\) && request\.confirmed !== true/);
  assert.match(request, /Cette action appareil exige une confirmation humaine explicite\./);
  assert.match(request, /request\.action === "shell"/);
  assert.match(request, /typeof request\.args\?\.command === "string" \? request\.args\.command : ""/);
  assert.match(request, /!command \|\| request\.exactConfirmation !== command/);
  assert.match(request, /La commande shell exacte doit être confirmée sans modification\./);
  assert.ok(request.indexOf("exactConfirmation") >= 0, "confirmation exacte");
});

test("l'action aboutit sur le contrôleur avec une demande figée et normalisée", () => {
  const request = block(usb, "export const requestManualUsbDeviceAction", "if (typeof window");
  assert.match(request, /if \(!manualUsbDeviceActionHandler\) \{/);
  assert.match(request, /Le contrôleur de la page Appareils n’est pas actif\./);
  assert.match(request, /manualUsbDeviceActionHandler\(Object\.freeze\(\{/);
  assert.match(request, /deviceKey: request\.deviceKey\?\.trim\(\) \|\| undefined,/);
  assert.match(request, /deviceId: request\.deviceId\?\.trim\(\) \|\| undefined,/);
  assert.match(request, /source: request\.source\.trim\(\),/);
  assert.ok(request.indexOf("Object.freeze") >= 0, "demande immuable");
});

test("la navigation assistée TikTok s'installe au chargement du module", () => {
  const install = usb.slice(usb.indexOf("if (typeof window"));
  assert.match(install, /void import\("\.\/tiktok-assisted-navigation"\)/);
  assert.match(install, /installTikTokAssistedNavigation\(\{/);
  assert.match(install, /requestManualAction: requestManualUsbDeviceAction,/);
  assert.match(install, /panelReadyEvent: USB_DEVICE_PANEL_READY_EVENT,/);
  assert.match(install, /slotSelector: USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR,/);
  assert.match(install, /\.catch\(\(error\) => \{\s*console\.error\("Navigation assistée TikTok indisponible", error\);/);
});

test("le runtime terminal est isolé et chargé paresseusement", () => {
  assert.match(terminal, /import \{ FitAddon \} from "@xterm\/addon-fit";/);
  assert.match(terminal, /import \{ Terminal \} from "@xterm\/xterm";/);
  assert.match(terminal, /import "@xterm\/xterm\/css\/xterm\.css";/);
  assert.match(terminal, /import \{ terminalThemeFor, type ThemeMode \} from "\.\/theme";/);
  assert.match(terminal, /import \{ openExternalHttpsUrl \} from "\.\/platform";/);
  assert.match(terminal, /Le moteur xterm est volontairement isole dans ce module\./);
  assert.match(terminal, /main\.ts` le charge\s*\n\s*\* avec import\(\) uniquement lorsqu'un terminal est effectivement ouvert/);
  assert.match(terminal, /export const createTerminalRuntime = \(theme: ThemeMode = "dark"\) => \{/);
});

test("la configuration xterm est bornée et claire", () => {
  const config = block(terminal, "export const createTerminalRuntime", "const fitAddon");
  assert.match(config, /new Terminal\(\{/);
  assert.match(config, /cursorBlink: true,/);
  assert.match(config, /cursorStyle: "bar",/);
  assert.match(config, /fontFamily: "Cascadia Mono, Consolas, monospace",/);
  assert.match(config, /fontSize: 13,/);
  assert.match(config, /lineHeight: 1\.15,/);
  assert.match(config, /scrollback: 500,/);
  assert.match(config, /linkHandler: \{/);
  assert.match(config, /activate: \(_event, uri\) => \{/);
  assert.match(config, /void openExternalHttpsUrl\(uri\)\.catch\(/);
  assert.match(config, /Impossible d'ouvrir ce lien :/);
  assert.match(config, /theme: terminalThemeFor\(theme\),/);
  assert.ok(config.indexOf("Vingt terminaux peuvent rester montes") >= 0, "commentaire scrollback");
  assert.ok(config.indexOf("claude auth login") >= 0, "OAuth relayé");
});

test("le runtime terminal charge le FitAddon et rend les deux", () => {
  const end = terminal.slice(terminal.indexOf("const fitAddon = new FitAddon()"));
  assert.match(end, /terminal\.loadAddon\(fitAddon\);/);
  assert.match(end, /return \{ terminal, fitAddon \};/);
  assert.ok(end.indexOf("fitAddon") >= 0, "addon fit");
});

test("main.ts charge le runtime terminal en import dynamique", () => {
  const lazy = main.match(/import\("\.\/terminal-runtime"\)/g) ?? [];
  assert.ok(lazy.length >= 1, "import dynamique du runtime terminal");
  assert.match(main, /createTerminalRuntime\(/);
});

test("remote-allocation modélise santé, charge et classement", () => {
  assert.match(allocation, /export type RemoteWorkloadKind = "chat" \| "terminal";/);
  assert.match(allocation, /export type RemoteAllocationHealth = \{/);
  assert.match(allocation, /ok: boolean;/);
  assert.match(allocation, /ready\?: boolean;/);
  assert.match(allocation, /draining\?: boolean;/);
  assert.match(allocation, /activeTerminals\?: number;/);
  assert.match(allocation, /activeChatTurns\?: number;/);
  assert.match(allocation, /capacity\?: number;/);
  assert.match(allocation, /availableAccountIds\?: string\[\];/);
  assert.match(allocation, /export type RemoteAllocationObservation<T> = \{/);
  assert.match(allocation, /node: T;/);
  assert.match(allocation, /health: RemoteAllocationHealth \| null;/);
  assert.match(allocation, /export type RankedRemoteAllocation<T> = RemoteAllocationObservation<T> & \{/);
  assert.match(allocation, /score: number;/);
  assert.match(allocation, /saturated: boolean;/);
  assert.match(allocation, /healthKnown: boolean;/);
  assert.match(allocation, /type PrioritizedNode = \{\s*priority: number;\s*\};/);
});

test("les nombres de charge sont bornés non négatifs", () => {
  const finite = block(allocation, "const finiteNonNegative", "const workloadCount");
  assert.match(finite, /const parsed = Number\(value\);/);
  assert.match(finite, /Number\.isFinite\(parsed\) && parsed > 0 \? parsed : 0/);
  assert.match(finite, /parsed : 0;/);
  assert.ok(finite.indexOf("> 0") >= 0, "strictement positif");
});

test("workloadCount additionne chats et terminaux actifs", () => {
  const count = block(allocation, "const workloadCount", "/\*\*");
  assert.match(count, /finiteNonNegative\(health\.activeChatTurns\)/);
  assert.match(count, /finiteNonNegative\(health\.activeTerminals\) \+ chats/);
  assert.ok(count.indexOf("activeChatTurns") >= 0, "tours de chat");
  assert.ok(count.indexOf("activeTerminals") >= 0, "terminaux");
});

test("un nœud sans sonde reste un dernier recours, jamais préféré", () => {
  const rank = block(allocation, "export const rankRemoteAllocations", "return ranked");
  assert.match(rank, /if \(health === null\) \{/);
  assert.match(rank, /score: Number\.POSITIVE_INFINITY,/);
  assert.match(rank, /saturated: true,/);
  assert.match(rank, /healthKnown: false,/);
  assert.match(rank, /tier: 2,/);
  assert.ok(rank.indexOf("POSITIVE_INFINITY") >= 0, "score infini");
});

test("un nœud indisponible, non prêt ou en drain est exclu", () => {
  const rank = block(allocation, "export const rankRemoteAllocations", "return ranked");
  assert.match(rank, /health\.ok === false \|\| health\.ready === false \|\| health\.draining === true/);
  assert.match(rank, /return \[\];/);
  assert.ok(rank.indexOf("ok: true` reste la preuve de disponibilité") >= 0, "commentaire ok");
});

test("un chat n'est envoyé que sur un nœud qui possède le compte demandé", () => {
  const rank = block(allocation, "export const rankRemoteAllocations", "return ranked");
  assert.match(rank, /accountId &&/);
  assert.match(rank, /Array\.isArray\(health\.availableAccountIds\)/);
  assert.match(rank, /!health\.availableAccountIds\.includes\(accountId\)/);
  assert.match(rank, /return \[\];/);
});

test("capacity 0 signifie sans plafond : jamais saturé par un simple décompte", () => {
  const rank = block(allocation, "export const rankRemoteAllocations", "return ranked");
  assert.match(rank, /const capacity = finiteNonNegative\(health\.capacity\);/);
  assert.match(rank, /const active = workloadCount\(health, workload\);/);
  assert.match(rank, /const saturated = capacity > 0 && active >= capacity;/);
  assert.match(rank, /score: \(capacity > 0 \? active \/ capacity : active\) \+ finiteNonNegative\(node\.priority\) \/ 100,/);
  assert.match(rank, /saturated,/);
  assert.match(rank, /healthKnown: true,/);
  assert.match(rank, /tier: saturated \? 1 : 0,/);
  assert.ok(rank.indexOf("sans plafond numerique") >= 0, "commentaire capacity 0");
});

test("le tri favorise les sains puis la charge puis la priorité", () => {
  const sort = allocation.slice(allocation.indexOf("return ranked"));
  assert.match(sort, /\.sort\(\(left, right\) =>/);
  assert.match(sort, /left\.tier - right\.tier \|\|/);
  assert.match(sort, /left\.score - right\.score \|\|/);
  assert.match(sort, /left\.node\.priority - right\.node\.priority/);
  assert.match(sort, /\.map\(\(\{ tier: _tier, \.\.\.entry \}\) => entry\);/);
  assert.ok(sort.indexOf("tier") >= 0, "palier de tri");
  assert.ok(sort.indexOf("score") >= 0, "score de tri");
});

test("le transport distant utilise le classement d'allocation", () => {
  const platform = read("../src/platform.ts");
  assert.match(platform, /rankRemoteAllocations\(observations, workload, accountId\)/);
  assert.match(platform, /import \{[^}]*rankRemoteAllocations[^}]*\} from "\.\/remote-allocation";/);
});
