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
  assert.ok(SCREEN_ACTIONS.size >= 11);
  assert.ok(SCREEN_ACTIONS.has("arm") && SCREEN_ACTIONS.has("disarm"));
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

test("les confirmations Windows locales couvrent les actions mutantes et l'armement", () => {
  assert.equal(requiresScreenApproval("click"), true);
  assert.equal(requiresScreenApproval("double_click"), true);
  assert.equal(requiresScreenApproval("right_click"), true);
  assert.equal(requiresScreenApproval("type"), true);
  assert.equal(requiresScreenApproval("arm"), true);
  assert.equal(requiresScreenApproval("disarm"), false);
  assert.equal(requiresScreenApproval("press", "Enter"), true);
  assert.equal(requiresScreenApproval("press", "Space"), true);
  assert.equal(requiresScreenApproval("press", "ArrowDown"), false);
  assert.equal(requiresScreenApproval("press", "F6"), false);
  assert.equal(requiresScreenApproval("press", "Ctrl+L"), false);
  assert.equal(requiresScreenApproval("press", "Win"), false);
  assert.equal(requiresScreenApproval("screenshot"), false);
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

  // L'armement est la confirmation unique (duree par defaut 10 min).
  const armed = await controller.handle({ ...base, action: "arm" });
  assert.equal(armed.ok, true);
  assert.equal(armed.armed, true);
  assert.equal(armed.minutes, 10);
  assert.equal(confirms.length, 2);
  assert.equal(confirms[1].action, "arm");

  // Pendant la fenetre armee : plus aucune popup pour les actions mutantes.
  await controller.handle({ ...base, action: "click", x: 10, y: 20 });
  await controller.handle({ ...base, action: "press", key: "Enter" });
  await controller.handle({ ...base, action: "type", text: "encore" });
  assert.equal(confirms.length, 2);

  // Expiration : la confirmation redevient obligatoire.
  clock += 10 * 60_000 + 1;
  await controller.handle({ ...base, action: "type", text: "apres expiration" });
  assert.equal(confirms.length, 3);

  // Desarmement explicite : meme avant expiration, plus aucune tolerance.
  const disarm = await controller.handle({ ...base, action: "disarm" });
  assert.equal(disarm.armed, false);
  await controller.handle({ ...base, action: "type", text: "apres desarmement" });
  assert.equal(confirms.length, 4);

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
  assert.ok(arm.description.includes("UNE seule confirmation"));
  assert.ok(arm.inputSchema.properties.minutes.maximum === 60);
  const disarm = tools.result.tools.find((tool) => tool.name === "screen_disarm");
  assert.deepEqual(disarm.inputSchema.properties, {});
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
