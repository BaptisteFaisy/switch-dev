import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

const loadUsbDeviceIntegrationModule = () => {
  const userActivation = { isActive: true };
  class TestEvent {
    constructor(type, isTrusted) {
      this.type = type;
      this.isTrusted = isTrusted;
    }
  }
  const transpiled = ts.transpileModule(read("src/usb-devices.ts"), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
    },
    fileName: "usb-devices.ts",
  }).outputText;
  const module = { exports: {} };
  const context = vm.createContext({
    module,
    exports: module.exports,
    Event: TestEvent,
    navigator: { userActivation },
  });
  vm.runInContext(transpiled, context, { filename: "usb-devices.cjs" });
  return {
    createEvent: (type, isTrusted) => new TestEvent(type, isTrusted),
    exports: module.exports,
    userActivation,
  };
};

const loadDeviceFleetModule = (initialResponse = { devices: [] }) => {
  let response = initialResponse;
  let manualActionHandler = null;
  const calls = [];
  const listeners = new Map();
  const source = read("src/device-fleet.ts").replace(
    /^import \{ invoke \} from "\.\/platform";\r?\n/,
    "const invoke = (...args) => globalThis.__deviceFleetInvoke(...args);\n",
  );
  const transpiled = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
    },
    fileName: "device-fleet.ts",
  }).outputText;
  const module = { exports: {} };
  class TestCustomEvent {
    constructor(type, options = {}) {
      this.type = type;
      this.detail = options.detail;
    }
  }
  const context = vm.createContext({
    module,
    exports: module.exports,
    require: (specifier) => {
      if (specifier !== "./usb-devices") throw new Error(`Module inattendu: ${specifier}`);
      return {
        USB_DEVICE_ASSISTED_NAVIGATION_SLOT_SELECTOR:
          '[data-usb-device-extension="assisted-navigation"]',
        USB_DEVICE_PANEL_READY_EVENT: "switch:usb-device-panel-ready",
        registerManualUsbDeviceActionHandler: (handler) => {
          if (manualActionHandler && manualActionHandler !== handler) {
            throw new Error("contrôleur déjà enregistré");
          }
          manualActionHandler = handler;
          return () => {
            if (manualActionHandler === handler) manualActionHandler = null;
          };
        },
      };
    },
    CustomEvent: TestCustomEvent,
    URL,
    console,
    window: {
      location: { origin: "https://switch.test" },
      setInterval: () => 1,
      clearInterval: () => undefined,
      addEventListener: (type, listener) => {
        const current = listeners.get(type) ?? new Set();
        current.add(listener);
        listeners.set(type, current);
      },
      removeEventListener: (type, listener) => listeners.get(type)?.delete(listener),
      dispatchEvent: (event) => {
        for (const listener of listeners.get(event.type) ?? []) listener(event);
        return true;
      },
    },
    document: {
      visibilityState: "visible",
      querySelector: () => null,
    },
    __deviceFleetInvoke: async (command, args = {}) => {
      calls.push({ command, args });
      return response;
    },
  });
  vm.runInContext(transpiled, context, { filename: "device-fleet.cjs" });
  return {
    exports: module.exports,
    calls,
    setResponse: (next) => {
      response = next;
    },
    dispatchManualAction: (request) => manualActionHandler?.(request),
    dispatchWindowAction: (request) => context.window.dispatchEvent(new TestCustomEvent(
      "switch:usb-device-action-request",
      { detail: request },
    )),
    hasManualActionHandler: () => manualActionHandler !== null,
  };
};

test("normalise une flotte Android et iOS issue du connecteur", () => {
  const { exports: fleet } = loadDeviceFleetModule();
  const snapshot = fleet.normalizeDeviceFleetSnapshot({
    connectorOnline: true,
    lastSeenAt: 1_777_777_777,
    error: "pont partiellement disponible",
    tools: {
      adb: { available: true, detail: "adb 1.0.41" },
      scrcpy: { available: false, detail: "introuvable" },
      iosSshKeyConfigured: true,
    },
    devices: [
      {
        deviceId: "android-1",
        platform: "android",
        status: "ready",
        capabilities: ["tap", "screenshot", "shell", "inconnue"],
      },
      {
        id: "ios-1",
        os: "iOS",
        udid: "00008110-001",
        name: "iPhone de test",
        connected: true,
        supportedActions: { tap: true, open_app: true, shell: false },
      },
    ],
  });

  assert.equal(snapshot.bridgeOnline, true);
  assert.equal(snapshot.updatedAt, 1_777_777_777_000);
  assert.equal(snapshot.warning, "pont partiellement disponible");
  assert.deepEqual(JSON.parse(JSON.stringify(snapshot.tools)), [
    { id: "adb", label: "ADB", available: true, detail: "adb 1.0.41" },
    { id: "scrcpy", label: "scrcpy", available: false, detail: "introuvable" },
    {
      id: "iosSshKeyConfigured",
      label: "Clé SSH iOS",
      available: true,
      detail: null,
    },
  ]);
  assert.equal(snapshot.devices.length, 2);
  assert.equal(snapshot.devices[0].platform, "android");
  assert.equal(snapshot.devices[0].name, "android-1", "le nom et le serial sont optionnels");
  assert.deepEqual([...snapshot.devices[0].capabilities], ["tap", "screenshot", "shell"]);
  assert.equal(snapshot.devices[1].platform, "ios");
  assert.equal(snapshot.devices[1].serial, "00008110-001");
  assert.deepEqual([...snapshot.devices[1].capabilities], ["tap", "open_app"]);
});

test("construit exactement les arguments convenus pour les neuf actions", () => {
  const { exports: fleet } = loadDeviceFleetModule();
  const cases = [
    ["info", undefined, false],
    ["screenshot", undefined, false],
    ["open_screen", undefined, true],
    ["tap", { x: 120, y: 340 }, true],
    ["swipe", { startX: 1, startY: 2, endX: 3, endY: 4, durationMs: 350 }, true],
    ["type_text", { text: "Bonjour" }, true],
    ["key_event", { key: "HOME" }, true],
    ["open_app", { appId: "com.exemple.app" }, true],
  ];
  for (const [action, args, confirmed] of cases) {
    const request = fleet.buildDeviceActionRequest(" device-1 ", action, args, confirmed);
    assert.equal(request.deviceId, "device-1");
    assert.equal(request.action, action);
    assert.equal(request.confirmed, confirmed);
    if (args === undefined) assert.equal("args" in request, false);
    else assert.deepEqual(JSON.parse(JSON.stringify(request.args)), args);
  }
  assert.deepEqual(
    JSON.parse(JSON.stringify(fleet.buildDeviceActionRequest(
      "ios-1",
      "shell",
      { command: "id" },
      true,
    ))),
    { deviceId: "ios-1", action: "shell", args: { command: "id" }, confirmed: true },
  );
});

test("le contrat d’extension USB exige un geste et une confirmation humaine", () => {
  const { createEvent, exports: integration, userActivation } = loadUsbDeviceIntegrationModule();
  const received = [];
  const release = integration.registerManualUsbDeviceActionHandler((request) => received.push(request));
  const trustedClick = () => createEvent("click", true);
  const syntheticClick = createEvent("click", false);

  assert.throws(
    () => integration.requestManualUsbDeviceAction(syntheticClick, {
      deviceKey: "android:one",
      action: "info",
      source: "test",
    }),
    /clic humain explicite/,
  );
  assert.throws(
    () => integration.requestManualUsbDeviceAction({ isTrusted: true, type: "click" }, {
      deviceKey: "android:one",
      action: "info",
      source: "test",
    }),
    /clic humain explicite/,
  );
  userActivation.isActive = false;
  assert.throws(
    () => integration.requestManualUsbDeviceAction(trustedClick(), {
      deviceKey: "android:one",
      action: "info",
      source: "test",
    }),
    /geste utilisateur n’est plus actif/,
  );
  userActivation.isActive = true;
  assert.throws(
    () => integration.requestManualUsbDeviceAction(trustedClick(), {
      deviceKey: "android:one",
      action: "tap",
      args: { x: 10, y: 20 },
      source: "test",
    }),
    /confirmation humaine explicite/,
  );
  assert.throws(
    () => integration.requestManualUsbDeviceAction(trustedClick(), {
      deviceKey: "android:one",
      action: "shell",
      args: { command: "id" },
      confirmed: true,
      exactConfirmation: "whoami",
      source: "test",
    }),
    /commande shell exacte/,
  );
  const acceptedClick = trustedClick();
  integration.requestManualUsbDeviceAction(acceptedClick, {
    deviceKey: "android:one",
    action: "tap",
    args: { x: 10, y: 20 },
    confirmed: true,
    source: "test",
  });
  assert.equal(received.length, 1);
  assert.equal(received[0].action, "tap");
  assert.equal(received[0].deviceKey, "android:one");
  assert.throws(
    () => integration.requestManualUsbDeviceAction(acceptedClick, {
      deviceKey: "android:one",
      action: "info",
      source: "test",
    }),
    /déjà autorisé une action appareil/,
  );
  release();
  assert.throws(
    () => integration.requestManualUsbDeviceAction(trustedClick(), {
      deviceKey: "android:one",
      action: "info",
      source: "test",
    }),
    /contrôleur de la page Appareils n’est pas actif/,
  );
});

test("refuse un événement DOM forgé et un couple deviceKey/deviceId incohérent", async () => {
  const harness = loadDeviceFleetModule({
    connectorOnline: true,
    devices: [
      { id: "one", platform: "android", status: "ready", capabilities: ["screenshot"] },
      { id: "two", platform: "android", status: "ready", capabilities: ["screenshot"] },
    ],
  });
  harness.exports.activateDeviceFleetPanel(() => undefined, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.hasManualActionHandler(), true);

  const request = {
    deviceKey: "android:one",
    deviceId: "one",
    action: "screenshot",
    source: "test",
  };
  harness.dispatchWindowAction(request);
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.calls.filter(({ command }) => command === "control_device").length, 0);

  assert.throws(
    () => harness.dispatchManualAction({ ...request, deviceId: "two" }),
    /ne correspond plus à la sélection/,
  );
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(harness.calls.filter(({ command }) => command === "control_device").length, 0);

  harness.dispatchManualAction(request);
  await new Promise((resolve) => setImmediate(resolve));
  const controlCalls = harness.calls.filter(({ command }) => command === "control_device");
  assert.equal(controlCalls.length, 1);
  assert.deepEqual(JSON.parse(JSON.stringify(controlCalls[0].args)), {
    deviceId: "one",
    action: "screenshot",
    confirmed: false,
  });

  harness.exports.deactivateDeviceFleetPanel();
  assert.equal(harness.hasManualActionHandler(), false);
});

test("refuse synchroniquement une action assistée hors ligne ou déjà en cours", async () => {
  const readyHarness = loadDeviceFleetModule({
    connectorOnline: true,
    devices: [
      { id: "one", platform: "android", status: "ready", capabilities: ["screenshot"] },
    ],
  });
  readyHarness.exports.activateDeviceFleetPanel(() => undefined, true);
  await new Promise((resolve) => setImmediate(resolve));
  const request = {
    deviceKey: "android:one",
    deviceId: "one",
    action: "screenshot",
    source: "test",
  };
  readyHarness.dispatchManualAction(request);
  assert.throws(
    () => readyHarness.dispatchManualAction(request),
    /déjà en cours/,
  );
  await new Promise((resolve) => setImmediate(resolve));
  readyHarness.exports.deactivateDeviceFleetPanel();

  const offlineHarness = loadDeviceFleetModule({
    connectorOnline: true,
    devices: [
      { id: "one", platform: "android", status: "offline", capabilities: ["screenshot"] },
    ],
  });
  offlineHarness.exports.activateDeviceFleetPanel(() => undefined, true);
  await new Promise((resolve) => setImmediate(resolve));
  assert.throws(
    () => offlineHarness.dispatchManualAction(request),
    /n’est plus prêt/,
  );
  offlineHarness.exports.deactivateDeviceFleetPanel();
});

test("limite strictement les médias aux captures image et écrans autorisés", () => {
  const { exports: fleet } = loadDeviceFleetModule();
  const base = "https://switch.test";
  assert.equal(
    fleet.safeDeviceMediaUrl("data:image/png;base64,aGVsbG8=", base, "screenshot"),
    "data:image/png;base64,aGVsbG8=",
  );
  assert.equal(fleet.safeDeviceMediaUrl("data:image/png;base64,aGVsbG8=", base, "screen"), null);
  assert.equal(
    fleet.safeDeviceMediaUrl("/api/device-fleet/screens/session_1", base, "screen"),
    "https://switch.test/api/device-fleet/screens/session_1",
  );
  assert.equal(fleet.safeDeviceMediaUrl("/api/settings", base, "screen"), null);
  assert.equal(fleet.safeDeviceMediaUrl("https://switch.test/", base, "screen"), null);
  assert.equal(
    fleet.safeDeviceMediaUrl("/api/device-fleet/screens/session_1?token=x", base, "screen"),
    null,
  );
  assert.equal(
    fleet.safeDeviceMediaUrl(
      "http://127.0.0.1:8000/embed.html?device=emulator-5554",
      base,
      "screen",
    ),
    "http://127.0.0.1:8000/embed.html?device=emulator-5554",
  );
  for (const rejected of [
    "http://127.0.0.1:8001/embed.html?device=a",
    "http://127.0.0.1:8000/other.html?device=a",
    "http://127.0.0.1:8000/embed.html?device=a&extra=1",
    "http://127.0.0.1:8000/embed.html?device=a&device=b",
    "http://127.0.0.1:8000/embed.html?device=%0Aevil",
    "http://127.0.0.1:8000/embed.html?device=device%20avec%20espace",
    "http://user:pass@127.0.0.1:8000/embed.html?device=a",
    "http://localhost:8000/embed.html?device=a#fragment",
    "https://evil.example/embed.html?device=a",
    "javascript:alert(1)",
  ]) {
    assert.equal(fleet.safeDeviceMediaUrl(rejected, base, "screen"), null, rejected);
  }
});

test("échappe les champs du connecteur dans la page multi-cartes", async () => {
  const harness = loadDeviceFleetModule({
    connectorOnline: true,
    devices: [
      {
        id: "android-evil",
        platform: "android",
        name: '<img src=x onerror="alert(1)">',
        serial: "serial</code><script>alert(2)</script>",
        status: '<svg onload="alert(3)">',
        ready: true,
      },
      { id: "ios-safe", platform: "ios", name: "iPhone", status: "ready", ready: true },
    ],
  });
  harness.exports.activateDeviceFleetPanel(() => undefined, true);
  await new Promise((resolve) => setImmediate(resolve));
  const html = harness.exports.renderDeviceFleetPanel({ remoteMode: true });
  harness.exports.deactivateDeviceFleetPanel();

  assert.match(html, /data-device-card="android:android-evil"/);
  assert.match(html, /data-device-card="ios:ios-safe"/);
  assert.doesNotMatch(html, /<img src=x/);
  assert.doesNotMatch(html, /<script>alert/);
  assert.match(html, /&lt;img src=x onerror=&quot;alert\(1\)&quot;&gt;/);
  assert.match(html, /serial&lt;\/code&gt;&lt;script&gt;alert\(2\)&lt;\/script&gt;/);
});

test("branche la page lazy, ses cycles de vie et les routes plateforme", () => {
  const main = read("src/main.ts");
  const platform = read("src/platform.ts");
  const view = read("src/device-fleet.ts");
  const integration = read("src/usb-devices.ts");
  const style = read("src/device-fleet.css");
  const initialStyles = [...main.matchAll(/^import\s+"\.\/([^"\n]+\.css)";\r?$/gm)]
    .map(([, path]) => read(`src/${path}`))
    .join("\n");

  assert.match(main, /type DeviceFleetModule = typeof import\("\.\/device-fleet"\)/);
  assert.match(main, /import\("\.\/device-fleet\.css"\)/);
  assert.match(main, /\| "devices"/);
  assert.match(main, /if \(view === "devices" && !deviceFleetModule\)/);
  assert.match(main, /data-view="devices"/);
  assert.match(main, /id="devicesToggle"/);
  assert.match(main, /case "devices":\s*return deviceFleetModule\?\.renderDeviceFleetPanel/);
  assert.match(main, /deviceFleetModule\?\.bindDeviceFleetUi/);
  assert.match(main, /deviceFleetModule\?\.activateDeviceFleetPanel\(render, isRemoteMode\(\)\)/);
  assert.match(main, /deviceFleetModule\?\.deactivateDeviceFleetPanel\(\)/);

  assert.match(platform, /case "list_control_devices":\s*return api<T>\("GET", "\/api\/device-fleet"\)/);
  assert.match(platform, /case "control_device":\s*return api<T>\("POST", "\/api\/device-fleet\/actions"/);
  assert.match(platform, /case "get_control_device_action":/);
  assert.match(platform, /`\/api\/device-fleet\/actions\/\$\{encodeURIComponent/);
  assert.match(platform, /deviceId: args\.deviceId/);
  assert.match(platform, /action: args\.action/);
  assert.match(platform, /confirmed: args\.confirmed/);

  for (const action of [
    "info",
    "screenshot",
    "open_screen",
    "tap",
    "swipe",
    "type_text",
    "key_event",
    "open_app",
    "shell",
  ]) {
    assert.match(view, new RegExp(`"${action}"`));
  }
  assert.match(view, /pendingShellConfirmations\.set/);
  assert.match(view, /action === "open_screen"/);
  assert.match(view, /performDeviceAction\(key, action as DeviceControlAction, args, true\)/);
  assert.match(view, /performDeviceAction\(key, "shell", \{ command \}, true\)/);
  assert.match(view, /record\.dataBase64/);
  assert.match(view, /const responseTone/);
  assert.match(view, /const waitForQueuedDeviceAction/);
  assert.match(view, /invoke<unknown>\("get_control_device_action"/);
  assert.match(view, /data-usb-device-extension="assisted-navigation"/);
  assert.match(view, /registerManualUsbDeviceActionHandler/);
  assert.doesNotMatch(view, /USB_DEVICE_ACTION_REQUEST_EVENT/);
  assert.match(view, /USB_DEVICE_PANEL_READY_EVENT/);
  assert.match(integration, /export const requestManualUsbDeviceAction/);
  assert.match(integration, /export const registerManualUsbDeviceActionHandler/);
  assert.doesNotMatch(integration, /switch:usb-device-action-request/);
  assert.match(integration, /trigger\.isTrusted/);
  assert.match(integration, /navigator\.userActivation/);
  assert.match(integration, /request\.exactConfirmation !== command/);
  assert.match(style, /\.device-fleet-grid/);
  assert.match(style, /\.device-fleet-extension-slot:empty/);
  assert.match(style, /@media \(max-width: 620px\)/);
  assert.match(style, /:root\[data-theme="light"\] \.device-fleet-panel/);
  assert.doesNotMatch(initialStyles, /\.device-fleet-/);
});
