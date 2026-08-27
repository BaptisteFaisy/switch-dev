import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import vm from "node:vm";
import ts from "typescript";

const read = (path) => readFileSync(new URL(`../${path}`, import.meta.url), "utf8");

const loadModule = () => {
  const mountedSelectors = [];
  const listeners = [];
  const transpiled = ts.transpileModule(read("src/tiktok-assisted-navigation.ts"), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
    },
    fileName: "tiktok-assisted-navigation.ts",
  }).outputText;
  const module = { exports: {} };
  const context = vm.createContext({
    module,
    exports: module.exports,
    require: (specifier) => {
      if (specifier === "./tiktok-assisted-navigation.css") return {};
      throw new Error(`Module inattendu: ${specifier}`);
    },
    window: {
      addEventListener: (type, listener) => listeners.push({ type, listener }),
    },
    document: {
      querySelectorAll: (selector) => {
        mountedSelectors.push(selector);
        return [];
      },
    },
  });
  vm.runInContext(transpiled, context, { filename: "tiktok-assisted-navigation.cjs" });
  return { exports: module.exports, listeners, mountedSelectors };
};

class FakeControl {
  constructor(dataset = {}) {
    this.dataset = dataset;
    this.focusCount = 0;
    this.listeners = new Map();
  }

  addEventListener(type, listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  emit(type, event = { type, isTrusted: true }) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }

  focus() {
    this.focusCount += 1;
  }
}

const loadInteractiveModule = () => {
  const listeners = new Map();
  const requests = [];
  let nextError = null;
  const summary = new FakeControl();
  const controls = {
    open: false,
    querySelector: (selector) => selector === "summary" ? summary : null,
    scrollIntoView: () => undefined,
  };
  const cardState = { android: true, ready: true };
  const card = {
    classList: {
      contains(value) {
        if (value === "platform-android") return cardState.android;
        if (value === "state-ready") return cardState.ready;
        return false;
      },
    },
    querySelector(selector) {
      if (selector === ".device-fleet-card-copy > strong") {
        return { textContent: "Pixel de test" };
      }
      if (selector === "[data-device-controls]") return controls;
      return null;
    },
  };
  const slot = {
    buttons: new Map(),
    card,
    dataset: { deviceId: "pixel-1", deviceKey: "android:pixel" },
    isConnected: true,
    markup: "",
    closest: (selector) => selector === "[data-device-card]" ? card : null,
    querySelector(selector) {
      return this.buttons.get(selector) ?? null;
    },
    querySelectorAll(selector) {
      if (selector !== "[data-tiktok-assisted-action]") return [];
      return [
        this.buttons.get('[data-tiktok-assisted-action="screenshot"]'),
        this.buttons.get('[data-tiktok-assisted-action="open_screen"]'),
      ].filter(Boolean);
    },
  };
  Object.defineProperty(slot, "innerHTML", {
    get: () => slot.markup,
    set: (value) => {
      slot.markup = value;
      slot.buttons = new Map([
        ["[data-tiktok-assisted-toggle]", new FakeControl()],
      ]);
      if (!value.includes('data-tiktok-assisted-action="screenshot"')) return;
      slot.buttons.set(
        '[data-tiktok-assisted-action="screenshot"]',
        new FakeControl({ tiktokAssistedAction: "screenshot" }),
      );
      slot.buttons.set(
        '[data-tiktok-assisted-action="open_screen"]',
        new FakeControl({ tiktokAssistedAction: "open_screen" }),
      );
      slot.buttons.set("[data-tiktok-assisted-controls]", new FakeControl());
      slot.buttons.set("[data-tiktok-assisted-skill]", new FakeControl());
    },
  });
  const transpiled = ts.transpileModule(read("src/tiktok-assisted-navigation.ts"), {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2022,
    },
    fileName: "tiktok-assisted-navigation.ts",
  }).outputText;
  const module = { exports: {} };
  const context = vm.createContext({
    module,
    exports: module.exports,
    require: (specifier) => {
      if (specifier === "./tiktok-assisted-navigation.css") return {};
      throw new Error(`Module inattendu: ${specifier}`);
    },
    console,
    document: {
      querySelector: () => null,
      querySelectorAll: (selector) => selector === "[data-assisted-slot]" ? [slot] : [],
    },
    window: {
      addEventListener: (type, listener) => listeners.set(type, listener),
      clearTimeout: () => undefined,
      setTimeout: () => 1,
    },
  });
  vm.runInContext(transpiled, context, { filename: "tiktok-assisted-navigation.cjs" });
  module.exports.installTikTokAssistedNavigation({
    requestManualAction: (event, request) => {
      if (nextError) {
        const error = nextError;
        nextError = null;
        throw error;
      }
      requests.push({ event, request });
      listeners.get("switch:test-panel-ready")?.();
    },
    panelReadyEvent: "switch:test-panel-ready",
    slotSelector: "[data-assisted-slot]",
  });
  return {
    cardState,
    listeners,
    requests,
    setNextError: (error) => {
      nextError = error;
    },
    slot,
  };
};

test("installe une seule fois l’assistant et remonte les slots à chaque panneau prêt", () => {
  const harness = loadModule();
  const options = {
    requestManualAction: () => undefined,
    panelReadyEvent: "switch:test-panel-ready",
    slotSelector: "[data-test-assisted-slot]",
  };

  harness.exports.installTikTokAssistedNavigation(options);
  harness.exports.installTikTokAssistedNavigation(options);
  assert.deepEqual(harness.mountedSelectors, [options.slotSelector]);
  assert.equal(harness.listeners.length, 1);
  assert.equal(harness.listeners[0].type, options.panelReadyEvent);

  harness.listeners[0].listener();
  assert.deepEqual(harness.mountedSelectors, [options.slotSelector, options.slotSelector]);
});

test("le module reste manuel et ouvre la fiche du skill par son identifiant stable", () => {
  const source = read("src/tiktok-assisted-navigation.ts");
  const integration = read("src/usb-devices.ts");
  const skillsView = read("src/skills-view.ts");

  assert.match(
    source,
    /TIKTOK_ASSISTED_NAVIGATION_SKILL_ID = "switch-tiktok-assisted-navigation"/,
  );
  assert.match(source, /\[data-skill-id=.*TIKTOK_ASSISTED_NAVIGATION_SKILL_ID/);
  assert.match(source, /details\.skill-details/);
  assert.match(source, /MutationObserver/);
  assert.match(skillsView, /data-skill-id="\$\{escapeHtml\(skill\.id\)\}"/);
  assert.match(source, /requestManualAction\(event,/);
  assert.match(source, /action !== "screenshot" && action !== "open_screen"/);
  assert.doesNotMatch(source, /setInterval|Math\.random/);
  assert.match(integration, /import\("\.\/tiktok-assisted-navigation"\)/);
});

test("journalise seulement une action acceptée et se ferme si Android devient indisponible", () => {
  const harness = loadInteractiveModule();
  harness.slot.querySelector("[data-tiktok-assisted-toggle]").emit("click");
  assert.match(harness.slot.innerHTML, /aria-expanded="true"/);
  assert.equal(
    harness.slot.querySelector("[data-tiktok-assisted-toggle]").focusCount,
    1,
  );

  harness.slot
    .querySelector('[data-tiktok-assisted-action="screenshot"]')
    .emit("click", { type: "click", isTrusted: true });
  assert.equal(harness.requests.length, 1);
  assert.match(harness.slot.innerHTML, /Capture transmise au contrôleur/);
  assert.equal(
    harness.slot.querySelector('[data-tiktok-assisted-action="screenshot"]').focusCount,
    1,
  );

  const staleButton = harness.slot.querySelector(
    '[data-tiktok-assisted-action="open_screen"]',
  );
  harness.cardState.ready = false;
  harness.listeners.get("switch:test-panel-ready")();
  assert.match(harness.slot.innerHTML, /Autorise et rends cet Android disponible/);
  assert.doesNotMatch(harness.slot.innerHTML, /tiktok-assisted-body/);
  staleButton.emit("click", { type: "click", isTrusted: true });
  assert.equal(harness.requests.length, 1, "aucune action ne part depuis un contrôle périmé");
});

test("remplace une transmission refusée par l’erreur réelle du contrôleur", () => {
  const harness = loadInteractiveModule();
  harness.slot.querySelector("[data-tiktok-assisted-toggle]").emit("click");
  harness.setNextError(new Error("Une action est déjà en cours sur cet appareil."));
  harness.slot
    .querySelector('[data-tiktok-assisted-action="screenshot"]')
    .emit("click", { type: "click", isTrusted: true });
  assert.equal(harness.requests.length, 0);
  assert.match(harness.slot.innerHTML, /Une action est déjà en cours sur cet appareil/);
  assert.doesNotMatch(harness.slot.innerHTML, /Capture transmise au contrôleur/);
});
