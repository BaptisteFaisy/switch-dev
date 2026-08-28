import test from "node:test";
import assert from "node:assert/strict";
import {
  MAX_AGENT_SCREEN_RESPONSE_BYTES,
  MAX_SCREENSHOT_BASE64,
  SAFE_SCREEN_KEYS,
  SCREEN_KEY_VK,
  SCREEN_ACTIONS,
  AgentScreenController,
  buildAgentScreenPowerShellScriptWithKeys,
  isSensitiveText,
  requiresScreenApproval,
  validateAgentScreenRequest,
} from "../scripts/agent-screen-core.mjs";
import {
  SCREEN_TOOLS,
  handleMcpRequest,
  screenSessionIdFromEnvironment,
} from "../scripts/cst-agent-screen-mcp.mjs";

const sessionId = "switch-test-session-123456";

const validRequest = (overrides = {}) => ({
  kind: "agent-screen",
  sessionId,
  action: "health",
  ...overrides,
});

test("les demandes ecran valides sont acceptees pour chaque action", () => {
  const cases = [
    { action: "health" },
    { action: "screenshot" },
    { action: "move", x: 100, y: 200 },
    { action: "click", x: -50, y: 10 },
    { action: "double_click", x: 0, y: 0 },
    { action: "right_click", x: 1920, y: 1080 },
    { action: "type", text: "Bonjour tout le monde" },
    { action: "press", key: "ArrowDown" },
    { action: "press", key: "Shift+Tab" },
    { action: "press", key: "F6" },
    { action: "press", key: "Ctrl+L" },
    { action: "press", key: "Win" },
    { action: "scroll", amount: 3 },
    { action: "scroll", amount: -2, x: 400, y: 300 },
    { action: "windows" },
    { action: "open", command: 'chrome "https://www.google.com/search?q=meteo+Paris"' },
    { action: "screenshot", window: 123456 },
    { action: "locate", text: "Type a message" },
    { action: "locate", text: "Reprendre", window: 555 },
    { action: "locate", text: "x", index: 2 },
    { action: "click", x: 10, y: 20, window: 987654 },
    { action: "type", text: "Bonjour", window: 555 },
    { action: "type", text: "Bonjour", window: 555, mode: "focus" },
    { action: "type", text: "Bonjour", window: 555, mode: "messages" },
    { action: "press", key: "Enter", window: 555, mode: "focus" },
    { action: "scroll", amount: 2, window: 555 },
    { action: "arm" },
    { action: "arm", minutes: 15 },
    { action: "disarm" },
  ];
  for (const value of cases) {
    const request = validateAgentScreenRequest(validRequest(value));
    assert.equal(request.kind, "agent-screen");
    assert.equal(request.sessionId, sessionId);
  }
});

test("les demandes ecran invalides sont refusees", () => {
  const invalid = [
    validRequest({ kind: "autre" }),
    validRequest({ sessionId: "court" }),
    validRequest({ sessionId: "avec espaces invalides 1234" }),
    validRequest({ sessionId: "" }),
    validRequest({ action: "inconnue" }),
    validRequest({ action: "click" }),
    validRequest({ action: "click", x: 1.5, y: 2 }),
    validRequest({ action: "click", x: "10", y: 2 }),
    validRequest({ action: "click", x: 100_001, y: 0 }),
    validRequest({ action: "move", x: 0 }),
    validRequest({ action: "press", key: "Ctrl+C" }),
    validRequest({ action: "press", key: "" }),
    validRequest({ action: "scroll", amount: 0 }),
    validRequest({ action: "scroll", amount: 21 }),
    validRequest({ action: "scroll", amount: 2, x: 5 }),
    validRequest({ action: "type", text: "" }),
    validRequest({ action: "type", text: "x".repeat(2001) }),
    validRequest({ action: "type", text: "avec\nretour" }),
    validRequest({ action: "arm", minutes: 0 }),
    validRequest({ action: "arm", minutes: 61 }),
    validRequest({ action: "arm", minutes: 2.5 }),
    validRequest({ action: "arm", minutes: "10" }),
    validRequest({ action: "open" }),
    validRequest({ action: "open", command: "x".repeat(513) }),
    validRequest({ action: "open", command: "chro\nme" }),
    validRequest({ action: "click", x: 1, y: 2, window: 0 }),
    validRequest({ action: "click", x: 1, y: 2, window: -5 }),
    validRequest({ action: "click", x: 1, y: 2, window: 1.5 }),
    validRequest({ action: "click", x: 1, y: 2, window: "abc" }),
    validRequest({ action: "screenshot", window: 99999999999 }),
    validRequest({ action: "locate" }),
    validRequest({ action: "locate", text: "" }),
    validRequest({ action: "locate", text: "   " }),
    validRequest({ action: "locate", text: "x".repeat(81) }),
    validRequest({ action: "locate", text: "x", index: -1 }),
    validRequest({ action: "locate", text: "x", index: 1.5 }),
    validRequest({ action: "locate", text: "x", index: 51 }),
    validRequest({ action: "locate", text: "x", index: "deux" }),
    validRequest({ action: "locate", text: "x", window: 0 }),
    validRequest({ action: "type", text: "x", window: 555, mode: "autre" }),
    validRequest({ action: "press", key: "Tab", window: 555, mode: "autre" }),
  ];
  for (const value of invalid) {
    assert.throws(() => validateAgentScreenRequest(value), undefined, JSON.stringify(value));
  }
});

test("les touches autorisees ont toutes une sequence de codes virtuels Windows", () => {
  for (const key of SAFE_SCREEN_KEYS) {
    const sequence = SCREEN_KEY_VK[key];
    assert.ok(Array.isArray(sequence) && sequence.length > 0, `touche sans sequence: ${key}`);
    assert.ok(sequence.every((vk) => Number.isInteger(vk) && vk > 0), `sequence invalide: ${key}`);
  }
  for (const key of Object.keys(SCREEN_KEY_VK)) {
    assert.ok(SAFE_SCREEN_KEYS.has(key), `code sans touche: ${key}`);
  }
  assert.deepEqual(SCREEN_KEY_VK["Ctrl+L"], [0x11, 0x4c]);
  assert.deepEqual(SCREEN_KEY_VK["Shift+Tab"], [0x10, 0x09]);
  assert.deepEqual(SCREEN_KEY_VK["Win"], [0x5b]);
  assert.deepEqual(SCREEN_KEY_VK["F6"], [0x75]);
  assert.ok(SCREEN_ACTIONS.size >= 13);
  assert.ok(SCREEN_ACTIONS.has("arm") && SCREEN_ACTIONS.has("disarm"));
  assert.ok(SCREEN_ACTIONS.has("windows") && SCREEN_ACTIONS.has("open"));
});

test("le texte sensible n'est jamais saisissable par le chat", () => {
  const sensitive = [
    "mon password est secret",
    "mot de passe: 1234",
    "code OTP 482913",
    "le pin est 1234",
    "cvv 123",
    "numero 4111111111111111",
    "IBAN FR7630006000011234567890189",
    "verification-code 123456",
  ];
  for (const text of sensitive) {
    assert.equal(isSensitiveText(text), true, `devrait etre refuse: ${text}`);
    assert.throws(() => validateAgentScreenRequest(validRequest({ action: "type", text })));
  }
  const acceptable = [
    "Bonjour, je suis pret",
    "rendez-vous a 15h30",
    "le code postal est 75001",
    "nom du dossier : photos vacances",
    "epingle a nourrice",
    "il passe par ici",
  ];
  for (const text of acceptable) {
    assert.equal(isSensitiveText(text), false, `devrait etre accepte: ${text}`);
    validateAgentScreenRequest(validRequest({ action: "type", text }));
  }
});

test("les confirmations Windows locales couvrent les actions mutantes, pas l'armement", () => {
  assert.equal(requiresScreenApproval("click"), true);
  assert.equal(requiresScreenApproval("double_click"), true);
  assert.equal(requiresScreenApproval("right_click"), true);
  assert.equal(requiresScreenApproval("type"), true);
  // L'armement est autorise par la demande explicite dans le chat, sans popup.
  assert.equal(requiresScreenApproval("arm"), false);
  assert.equal(requiresScreenApproval("disarm"), false);
  assert.equal(requiresScreenApproval("press", "Enter"), true);
  assert.equal(requiresScreenApproval("press", "Space"), true);
  assert.equal(requiresScreenApproval("press", "ArrowDown"), false);
  assert.equal(requiresScreenApproval("press", "F6"), false);
  assert.equal(requiresScreenApproval("press", "Ctrl+L"), false);
  assert.equal(requiresScreenApproval("press", "Win"), false);
  assert.equal(requiresScreenApproval("screenshot"), false);
  assert.equal(requiresScreenApproval("locate"), false);
  assert.equal(requiresScreenApproval("move"), false);
  assert.equal(requiresScreenApproval("scroll"), false);
});

test("la session armee passe sans confirmation jusqu'a expiration ou desarmement", async () => {
  const confirms = [];
  let clock = 1_000_000;
  const controller = new AgentScreenController({
    confirmAction: async (metadata) => {
      confirms.push(metadata);
      return true;
    },
    nowImpl: () => clock,
    executeImpl: async (request) => ({
      ok: true,
      action: request.action,
      screenshot: "aGVsbG8=",
      width: 1,
      height: 1,
      screenWidth: 1,
      screenHeight: 1,
    }),
  });
  const base = { kind: "agent-screen", sessionId: "switch-armed-test-123456" };

  // Session non armee : chaque action mutante demande la confirmation locale.
  await controller.handle({ ...base, action: "type", text: "Bonjour" });
  assert.equal(confirms.length, 1);
  assert.equal(confirms[0].action, "type");

  // L'armement ne demande aucune popup (autorise par la demande dans le chat).
  const armed = await controller.handle({ ...base, action: "arm" });
  assert.equal(armed.ok, true);
  assert.equal(armed.armed, true);
  assert.equal(armed.minutes, 10);
  assert.equal(confirms.length, 1);

  // Pendant la fenetre armee : plus aucune popup pour les actions mutantes.
  await controller.handle({ ...base, action: "click", x: 10, y: 20 });
  await controller.handle({ ...base, action: "press", key: "Enter" });
  await controller.handle({ ...base, action: "type", text: "encore" });
  assert.equal(confirms.length, 1);

  // Expiration : la confirmation redevient obligatoire.
  clock += 10 * 60_000 + 1;
  await controller.handle({ ...base, action: "type", text: "apres expiration" });
  assert.equal(confirms.length, 2);

  // Desarmement explicite : meme avant expiration, plus aucune tolerance.
  const disarm = await controller.handle({ ...base, action: "disarm" });
  assert.equal(disarm.armed, false);
  await controller.handle({ ...base, action: "type", text: "apres desarmement" });
  assert.equal(confirms.length, 3);

  await controller.close();
});

test("le script PowerShell compile les actions et le mapping des touches", () => {
  const script = buildAgentScreenPowerShellScriptWithKeys();
  assert.ok(script.includes("public static class ScreenAgent"));
  assert.ok(script.includes("SendInput"));
  assert.ok(script.includes("CopyFromScreen"));
  assert.ok(script.includes("ConvertTo-Json"));
  assert.ok(script.includes("PressSequence"));
  assert.ok(script.includes("GetVkSequence"));
  // Mode arriere-plan : ciblage de fenetre sans focus.
  assert.ok(script.includes("PostMessage"));
  assert.ok(script.includes("PrintWindow"));
  assert.ok(script.includes("LaunchNoFocus"));
  assert.ok(script.includes("ListWindowsText"));
  assert.ok(script.includes("ClickWindowClient"));
  assert.ok(script.includes("TypeToWindow"));
  assert.ok(script.includes("PressToWindow"));
  // Saisie universelle : focus clavier temporaire sans changement d'ordre Z.
  assert.ok(script.includes("AttachThreadInput"));
  assert.ok(script.includes("TypeFocusSteal"));
  assert.ok(script.includes("PressFocusSteal"));
  assert.ok(script.includes("ScrollWindowClient"));
  // Repérage visuel : OCR natif + dispatch locate.
  assert.ok(script.includes("Find-OcrCandidates"));
  assert.ok(script.includes("'locate'"));
  assert.ok(script.includes("RecognizeAsync"));
  assert.ok(script.includes('if (key == "ArrowDown") return new int[] { 0x28 };'));
  assert.ok(script.includes('if (key == "Shift+Tab") return new int[] { 0x10, 0x9 };'));
  assert.ok(script.includes('if (key == "Ctrl+L") return new int[] { 0x11, 0x4c };'));
  assert.ok(script.includes('if (key == "Win") return new int[] { 0x5b };'));
  assert.ok(script.includes('if (key == "F6") return new int[] { 0x75 };'));
  assert.ok(!script.includes("KeyDownUp"));
  assert.ok(!script.includes("SCREEN_KEY_VK_MAPPING"));
  assert.ok(!script.includes("${"));
  // Le plafond de capture est epingle dans le script PowerShell genere.
  assert.ok(script.includes(`$MAX_SCREENSHOT_BASE64 = ${MAX_SCREENSHOT_BASE64}`));
});

test("les plafonds de taille laissent la place a une capture d'ecran", () => {
  // Le JSON contient le base64 une seule fois (pas de re-encodage), plus les
  // metadonnees : la reponse du broker doit le laisser passer.
  const overhead = 64 * 1024;
  assert.ok(
    MAX_AGENT_SCREEN_RESPONSE_BYTES >= MAX_SCREENSHOT_BASE64 + overhead,
    "la reponse du broker est trop juste pour la capture"
  );
});

test("le serveur MCP ecran expose ses outils et leur schema", async () => {
  const tools = await handleMcpRequest({
    jsonrpc: "2.0",
    id: 1,
    method: "tools/list",
  });
  const names = tools.result.tools.map((tool) => tool.name);
  assert.deepEqual(names, SCREEN_TOOLS.map((tool) => tool.name));
  const click = tools.result.tools.find((tool) => tool.name === "screen_click");
  assert.deepEqual(click.inputSchema.required, ["x", "y"]);
  assert.equal(click.inputSchema.additionalProperties, false);
  const press = tools.result.tools.find((tool) => tool.name === "screen_press");
  assert.ok(press.inputSchema.properties.key.enum.includes("Enter"));
  assert.ok(press.inputSchema.properties.key.enum.includes("F6"));
  assert.ok(press.inputSchema.properties.key.enum.includes("Ctrl+L"));
  assert.ok(press.inputSchema.properties.key.enum.includes("Win"));
  const arm = tools.result.tools.find((tool) => tool.name === "screen_arm");
  assert.ok(arm.description.includes("AUCUNE popup"));
  assert.ok(arm.description.includes("demande explicite"));
  assert.ok(arm.inputSchema.properties.minutes.maximum === 60);
  const disarm = tools.result.tools.find((tool) => tool.name === "screen_disarm");
  assert.deepEqual(disarm.inputSchema.properties, {});
  const windows = tools.result.tools.find((tool) => tool.name === "screen_windows");
  assert.ok(windows);
  assert.deepEqual(windows.inputSchema.properties, {});
  const locate = tools.result.tools.find((tool) => tool.name === "screen_locate");
  assert.ok(locate);
  assert.deepEqual(locate.inputSchema.required, ["text"]);
  assert.equal(locate.inputSchema.properties.index.maximum, 50);
  const open = tools.result.tools.find((tool) => tool.name === "screen_open");
  assert.deepEqual(open.inputSchema.required, ["command"]);
  assert.ok(open.description.includes("SANS prendre le focus"));
  // Le parametre window est accepte sur les actions ciblables.
  assert.ok(click.inputSchema.properties.window);
  assert.ok(!tools.result.tools.find((tool) => tool.name === "screen_move").inputSchema.properties.window);
});

test("le serveur MCP retourne la capture en image avec l'echelle", async () => {
  const result = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 2,
      method: "tools/call",
      params: {
        name: "screen_snapshot",
        arguments: {},
      },
    },
    {
      screenSessionId: sessionId,
      invoke: async (request) => ({
        ok: true,
        screenshot: "aGVsbG8=",
        width: 1600,
        height: 900,
        screenWidth: 2560,
        screenHeight: 1440,
        cursorX: 100,
        cursorY: 100,
        activeWindow: "Explorateur",
      }),
    }
  );
  assert.equal(result.result.isError, undefined);
  const image = result.result.content.find((part) => part.type === "image");
  assert.ok(image);
  assert.equal(image.data, "aGVsbG8=");
  assert.equal(image.mimeType, "image/jpeg");
  const text = result.result.content.find((part) => part.type === "text").text;
  assert.ok(text.includes("2560x1440"));
  assert.ok(text.includes("echelle 1.60"));
  // Le payload image ne doit pas etre duplique dans le texte.
  assert.ok(!text.includes("aGVsbG8="));
});

test("le serveur MCP relaie l'action avec la bonne session et marque les erreurs", async () => {
  let received = null;
  const ok = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 3,
      method: "tools/call",
      params: { name: "screen_click", arguments: { x: 10, y: 20 } },
    },
    {
      screenSessionId: sessionId,
      invoke: async (request) => {
        received = request;
        return { ok: true, action: "click", x: 10, y: 20, screenshot: "aGVsbG8=", width: 1, height: 1, screenWidth: 1, screenHeight: 1 };
      },
    }
  );
  assert.equal(received.kind, "agent-screen");
  assert.equal(received.action, "click");
  assert.equal(received.sessionId, sessionId);
  assert.equal(received.x, 10);
  assert.equal(received.y, 20);

  const armedCall = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 5,
      method: "tools/call",
      params: { name: "screen_arm", arguments: { minutes: 15 } },
    },
    {
      screenSessionId: sessionId,
      invoke: async (request) => {
        received = request;
        return { ok: true, armed: true, minutes: 15 };
      },
    }
  );
  assert.equal(received.action, "arm");
  assert.equal(received.minutes, 15);
  assert.ok(armedCall.result.content[0].text.includes("armed"));

  const failed = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 4,
      method: "tools/call",
      params: { name: "screen_press", arguments: { key: "Escape" } },
    },
    {
      screenSessionId: sessionId,
      invoke: async () => ({ ok: false, error: "Action refusee." }),
    }
  );
  assert.equal(failed.result.isError, true);
  assert.ok(failed.result.content[0].text.includes("Action refusee."));
});

test("les demandes navigateur valides sont acceptees et propagees", () => {
  const cases = [
    { action: "browser", method: "list", window: 123456 },
    { action: "browser", method: "list", cdpPort: 9222 },
    { action: "browser", method: "screenshot", cdpPort: 9223 },
    { action: "browser", method: "eval", window: 123456, cdpPort: 9222, expression: "document.title" },
    { action: "browser", method: "navigate", window: 123456, url: "https://fr.wikipedia.org" },
    { action: "browser", method: "eval", window: 123456, expression: "document.title" },
    { action: "browser", method: "click", window: 123456, selector: "a[href*='wikipedia']" },
    { action: "browser", method: "type", window: 123456, selector: "input[name=q]", text: "meteo Paris" },
    { action: "browser", method: "key", window: 123456, key: "Enter" },
  ];
  for (const value of cases) {
    const request = validateAgentScreenRequest(validRequest(value));
    assert.equal(request.action, "browser");
    assert.equal(request.method, value.method);
    assert.equal(request.window, value.window);
  }
});

test("les demandes navigateur invalides sont refusees", () => {
  const invalid = [
    validRequest({ action: "browser" }),
    validRequest({ action: "browser", method: "inconnue", window: 123456 }),
    validRequest({ action: "browser", method: "list" }),
    validRequest({ action: "browser", method: "list", cdpPort: 80 }),
    validRequest({ action: "browser", method: "list", cdpPort: 70000 }),
    validRequest({ action: "browser", method: "list", cdpPort: "http" }),
    validRequest({ action: "browser", method: "list", window: -1 }),
    validRequest({ action: "browser", method: "navigate", window: 123456 }),
    validRequest({ action: "browser", method: "navigate", window: 123456, url: "ftp://x" }),
    validRequest({ action: "browser", method: "navigate", window: 123456, url: "javascript:alert(1)" }),
    validRequest({ action: "browser", method: "eval", window: 123456 }),
    validRequest({ action: "browser", method: "click", window: 123456 }),
    validRequest({ action: "browser", method: "type", window: 123456, selector: "input" }),
    validRequest({ action: "browser", method: "type", window: 123456, text: "sans selecteur" }),
    validRequest({ action: "browser", method: "type", window: 123456, selector: "input", text: "code OTP 482913" }),
    validRequest({ action: "browser", method: "type", window: 123456, selector: "input", text: "ligne\nretour" }),
    validRequest({ action: "browser", method: "key", window: 123456, key: "Ctrl+C" }),
    validRequest({ action: "browser", method: "key", window: 123456, key: "Win" }),
  ];
  for (const value of invalid) {
    assert.throws(() => validateAgentScreenRequest(value), undefined, JSON.stringify(value));
  }
});

test("les mutations navigateur exigent la confirmation, pas la lecture", () => {
  assert.equal(requiresScreenApproval("browser:list"), false);
  assert.equal(requiresScreenApproval("browser:screenshot"), false);
  assert.equal(requiresScreenApproval("browser:navigate"), true);
  assert.equal(requiresScreenApproval("browser:eval"), true);
  assert.equal(requiresScreenApproval("browser:click"), true);
  assert.equal(requiresScreenApproval("browser:type"), true);
  assert.equal(requiresScreenApproval("browser:key"), true);
});

test("le script PowerShell embarque le pilotage CDP du navigateur", () => {
  const script = buildAgentScreenPowerShellScriptWithKeys();
  assert.ok(script.includes("Get-CdpPort"));
  assert.ok(script.includes("Invoke-Cdp"));
  assert.ok(script.includes("Invoke-CdpKey"));
  assert.ok(script.includes("Get-CdpPageTarget"));
  assert.ok(script.includes("ClientWebSocket"));
  assert.ok(script.includes("Page.captureScreenshot"));
  assert.ok(script.includes("Page.navigate"));
  assert.ok(script.includes("Runtime.evaluate"));
  assert.ok(script.includes("Input.dispatchKeyEvent"));
  assert.ok(script.includes("DevToolsActivePort"));
  assert.ok(script.includes("--remote-debugging-port=0"));
  // Anti-bot : masquage de navigator.webdriver + consentement Google.
  assert.ok(script.includes("--disable-blink-features=AutomationControlled"));
  assert.ok(script.includes("Invoke-CdpStealth"));
  assert.ok(script.includes("Object.defineProperty(navigator, 'webdriver'"));
  assert.ok(script.includes("#L2AGLb"));
});

test("les outils navigateur MCP existent et relaient la methode CDP", async () => {
  const tools = await handleMcpRequest({ jsonrpc: "2.0", id: 1, method: "tools/list" });
  const names = tools.result.tools.map((tool) => tool.name);
  for (const expected of ["browser_list", "browser_screenshot", "browser_navigate", "browser_eval", "browser_click", "browser_type", "browser_key"]) {
    assert.ok(names.includes(expected), `outil manquant: ${expected}`);
  }
  const navigate = tools.result.tools.find((tool) => tool.name === "browser_navigate");
  assert.deepEqual(navigate.inputSchema.required, ["window", "url"]);
  assert.equal(navigate.inputSchema.additionalProperties, false);
  const type = tools.result.tools.find((tool) => tool.name === "browser_type");
  assert.deepEqual(type.inputSchema.required, ["window", "selector", "text"]);

  let received = null;
  const result = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 7,
      method: "tools/call",
      params: { name: "browser_navigate", arguments: { window: 123456, url: "https://fr.wikipedia.org" } },
    },
    {
      screenSessionId: sessionId,
      invoke: async (request) => {
        received = request;
        return { ok: true, navigated: true, url: "https://fr.wikipedia.org" };
      },
    }
  );
  assert.equal(received.action, "browser");
  assert.equal(received.method, "navigate");
  assert.equal(received.window, 123456);
  assert.equal(received.url, "https://fr.wikipedia.org");
  assert.ok(result.result.content[0].text.includes("navigated"));

  // Capture CDP : image renvoyee avec le titre de l'onglet.
  const shot = await handleMcpRequest(
    {
      jsonrpc: "2.0",
      id: 8,
      method: "tools/call",
      params: { name: "browser_screenshot", arguments: { window: 123456 } },
    },
    {
      screenSessionId: sessionId,
      invoke: async () => ({
        ok: true,
        pageCapture: true,
        window: 123456,
        screenshot: "aGVsbG8=",
        title: "Wikipedia",
        url: "https://fr.wikipedia.org",
      }),
    }
  );
  const image = shot.result.content.find((part) => part.type === "image");
  assert.ok(image);
  const text = shot.result.content.find((part) => part.type === "text").text;
  assert.ok(text.includes("Wikipedia"));
  assert.ok(text.includes("sans focus"));
});

test("l'identifiant de session ecran est stable depuis la graine", () => {
  const environment = { CST_AGENT_SCREEN_SESSION_SEED: "terminal-42" };
  const first = screenSessionIdFromEnvironment(environment);
  const second = screenSessionIdFromEnvironment(environment);
  assert.equal(first, second);
  assert.match(first, /^switch-[A-Za-z0-9_-]+$/);
  assert.notEqual(
    screenSessionIdFromEnvironment({}),
    screenSessionIdFromEnvironment({})
  );
});
