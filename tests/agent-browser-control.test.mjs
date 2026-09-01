import test from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  AgentBrowserController,
  validatedWebUrl,
  validateAgentBrowserRequest,
} from "../scripts/agent-browser-core.mjs";

const sessionId = "switch-browser-test-12345";

const validRequest = (overrides = {}) => ({
  kind: "agent-browser",
  sessionId,
  action: "health",
  ...overrides,
});

// Faux navigateur : un nouveauPage cree une page qui ne repond jamais
// (goto bloque) ou une page normale selon la strategie passee. En mode
// hangFirst, seule la premiere page bloque (les suivantes sont normales).
const fakeBrowser = ({ hang = false, hangFirst = false } = {}) => {
  let newPageCount = 0;
  const hangPage = {
    isClosed: () => false,
    close: async () => {},
    setDefaultTimeout: () => {},
    on: () => {},
    goto: () => new Promise(() => undefined),
  };
  const okPage = {
    isClosed: () => false,
    close: async () => {},
    setDefaultTimeout: () => {},
    on: () => {},
    goto: async () => {},
    evaluate: async () => ({ url: "https://example.com", title: "Example Domain", text: "hello" }),
    locator: () => ({ elementHandles: async () => [] }),
    bringToFront: async () => {},
    keyboard: { press: async () => {} },
    waitForEvent: () => ({ catch: async () => null }),
    waitForLoadState: async () => {},
    goBack: async () => {},
    url: () => "https://example.com",
    on: () => {},
  };
  const context = {
    pages: () => [],
    newPage: async () => {
      newPageCount += 1;
      if (hangFirst) return newPageCount === 1 ? hangPage : okPage;
      return hang ? hangPage : okPage;
    },
    route: async () => {},
    on: () => {},
    once: () => {},
    close: async () => {},
  };
  return { context, counts: () => newPageCount };
};

const buildController = ({ hang = false, hangFirst = false, requestTimeoutMilliseconds = 300 } = {}) => {
  const browser = fakeBrowser({ hang, hangFirst });
  const controller = new AgentBrowserController({
    importPlaywright: async () => ({ chromium: { launchPersistentContext: async () => browser.context } }),
    executablePath: "C:/fake/chrome-agent-test.exe",
    profileDirectory: mkdtempSync(join(tmpdir(), "agent-browser-test-")),
    createProxy: async () => ({ url: "http://127.0.0.1:1", close: async () => {} }),
    requestTimeoutMilliseconds,
  });
  return { controller, browser };
};

const closeController = async (controller, directory) => {
  await controller.close().catch(() => undefined);
  rmSync(directory, { recursive: true, force: true });
};

test("les demandes navigateur valides sont acceptees", () => {
  const cases = [
    { action: "open", url: "https://fr.wikipedia.org/wiki/Switch" },
    { action: "snapshot" },
    { action: "back" },
    { action: "close" },
    { action: "health" },
    { action: "click", ref: "s1-e1" },
    { action: "fill", ref: "s1-e2", text: "Bonjour" },
    { action: "select", ref: "s1-e3", value: "Paris" },
    { action: "press", key: "Enter" },
    { action: "press", key: "Tab", ref: "s1-e4" },
  ];
  for (const value of cases) {
    const request = validateAgentBrowserRequest(validRequest(value));
    assert.equal(request.kind, "agent-browser");
    assert.equal(request.sessionId, sessionId);
  }
});

test("les demandes navigateur invalides sont refusees", () => {
  const invalid = [
    validRequest({ kind: "autre" }),
    validRequest({ sessionId: "court" }),
    validRequest({ action: "click", ref: "x9-aaa" }),
    validRequest({ action: "fill", ref: "s1-e1" }),
    validRequest({ action: "press", key: "Ctrl+W" }),
    validRequest({ action: "open", url: "ftp://exemple.fr" }),
    validRequest({ action: "open", url: "http://localhost:8080" }),
  ];
  for (const value of invalid) {
    assert.throws(() => validateAgentBrowserRequest(value));
  }
  assert.throws(() => validatedWebUrl("http://192.168.1.1/admin"));
});

test("une action realiste s'execute avec une page persistante", async () => {
  const { controller, browser } = buildController();
  const directory = controller.profileDirectory;
  try {
    const health = await controller.handle(validRequest());
    assert.equal(health.ok, true);
    const opened = await controller.handle(validRequest({ action: "open", url: "https://example.com" }));
    assert.equal(opened.ok, true);
    assert.equal(opened.page.title, "Example Domain");
    const snapshot = await controller.handle(validRequest({ action: "snapshot" }));
    assert.equal(snapshot.ok, true);
    // La meme page persiste entre les actions : une seule creation.
    assert.equal(browser.counts(), 1);
  } finally {
    await closeController(controller, directory);
  }
});

test("une action qui depasse le delai libere la session au lieu de bloquer", async () => {
  const { controller, browser } = buildController({ hangFirst: true });
  const directory = controller.profileDirectory;
  try {
    const started = Date.now();
    await assert.rejects(
      controller.handle(validRequest({ action: "open", url: "https://example.com" })),
      /mis trop de temps/
    );
    assert.ok(Date.now() - started < 5000, "le chien de garde doit couper la page bloquee");
    // La session bloquee est liberee : l'action suivante ouvre une page neuve.
    const second = await controller.handle(validRequest({ action: "snapshot" }));
    assert.equal(second.ok, true);
    assert.equal(browser.counts(), 2);
  } finally {
    await closeController(controller, directory);
  }
});

test("health et close repondent sans passer par le navigateur", async () => {
  const { controller, browser } = buildController({ hang: true });
  const directory = controller.profileDirectory;
  try {
    const health = await controller.handle(validRequest());
    assert.equal(health.ok, true);
    const closed = await controller.handle(validRequest({ action: "close" }));
    assert.equal(closed.ok, true);
    assert.equal(closed.closed, true);
    assert.equal(browser.counts(), 0);
  } finally {
    await closeController(controller, directory);
  }
});