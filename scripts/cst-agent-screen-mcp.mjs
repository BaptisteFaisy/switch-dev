import { createHash, randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

// La reponse d'une capture d'ecran (base64 + metadonnees) tient dans cette
// limite de ligne ; les demandes restent petites.
const MAX_LINE_BYTES = 1_300_000;
const DEFAULT_PROTOCOL_VERSION = "2024-11-05";

export const screenSessionIdFromEnvironment = (
  environment = process.env,
  fallbackUuid = randomUUID,
) => {
  const seed = String(environment.CST_AGENT_SCREEN_SESSION_SEED || "").trim();
  if (seed && seed.length <= 4096) {
    const digest = createHash("sha256").update(seed, "utf8").digest("base64url");
    return `switch-${digest}`;
  }
  return `switch-${fallbackUuid()}`;
};

const sessionId = screenSessionIdFromEnvironment();

const SAFE_KEY_ENUM = [
  "Enter",
  "Escape",
  "Tab",
  "Shift+Tab",
  "Backspace",
  "Delete",
  "ArrowUp",
  "ArrowDown",
  "ArrowLeft",
  "ArrowRight",
  "Home",
  "End",
  "PageUp",
  "PageDown",
  "Space",
  "F6",
  "Ctrl+L",
  "Win",
];

const coordinateDescription =
  "Coordonnee en pixels de l'ecran REEL (pas de l'image renvoyee). Multiplier une position vue sur l'image par screenWidth/width (voir le texte de screen_snapshot).";

export const SCREEN_TOOLS = [
  {
    name: "screen_snapshot",
    description:
      "Capture l'ecran complet du PC (tous les moniteurs) dans la fenetre visible, re-echantillonne en JPEG, et renvoie l'image avec sa resolution reelle, l'echelle et la fenetre active. Utiliser en premier pour voir l'etat de l'ecran avant toute action.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "screen_move",
    description: "Deplace la souris jusqu'au point (x, y) de l'ecran reel sans cliquer, puis renvoie une capture.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_click",
    description:
      "Clique gauche au point (x, y) de l'ecran reel, puis renvoie une capture. Une confirmation Windows locale s'affiche avant le clic. Pour une action sensible (paiement, suppression, envoi), demander d'abord une confirmation explicite a l'utilisateur.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_double_click",
    description:
      "Double-clic gauche au point (x, y) de l'ecran reel (ouvrir un dossier, lancer une application), puis renvoie une capture. Confirmation Windows locale avant le clic.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_right_click",
    description:
      "Clic droit au point (x, y) de l'ecran reel pour ouvrir un menu contextuel, puis renvoie une capture. Confirmation Windows locale avant le clic.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_type",
    description:
      "Saisit un texte non sensible dans l'application ou le champ actuellement actif, puis renvoie une capture. Les mots de passe, codes OTP et donnees bancaires sont toujours refuses et doivent etre saisis manuellement par l'utilisateur. Confirmation Windows locale avant la saisie.",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string", description: "Texte non sensible a saisir (2000 caracteres max)." },
      },
      required: ["text"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_press",
    description:
      "Appuie sur une touche de navigation sure (Enter, Escape, Tab, fleches, etc.) dans la fenetre active, puis renvoie une capture. Entree et Espace exigent une confirmation Windows locale.",
    inputSchema: {
      type: "object",
      properties: {
        key: { type: "string", enum: SAFE_KEY_ENUM, description: "Touche de navigation autorisee." },
      },
      required: ["key"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_scroll",
    description:
      "Fait defiler la fenetre sous le curseur (molette), puis renvoie une capture. amount positif = vers le haut, negatif = vers le bas (une cran = 1). Positionne d'abord le curseur si x et y sont fournis.",
    inputSchema: {
      type: "object",
      properties: {
        amount: { type: "integer", description: "Nombre de crans de molette, entre -20 et 20, non nul." },
        x: { type: "integer", description: "Position X optionnelle du curseur avant defilement." },
        y: { type: "integer", description: "Position Y optionnelle du curseur avant defilement." },
      },
      required: ["amount"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_arm",
    description:
      "Arme la session de controle d'ecran : UNE seule confirmation Windows locale s'affiche, puis toutes les actions de cette session (clics, saisies, touches) passent SANS confirmation jusqu'a expiration ou desarmement. A n'utiliser qu'apres une demande explicite de l'utilisateur (ex. « fais-le tout seul ») et pour une mission bornee dans le temps.",
    inputSchema: {
      type: "object",
      properties: {
        minutes: { type: "integer", minimum: 1, maximum: 60, description: "Duree de la fenetre armee en minutes (defaut 10, max 60)." },
      },
      additionalProperties: false,
    },
  },
  {
    name: "screen_disarm",
    description: "Desarme immediatement la session : les prochaines actions mutantes exigent de nouveau la confirmation Windows locale.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "screen_health",
    description: "Verifie que le controle d'ecran du PC est pret et joignable.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
];

const TOOL_ACTIONS = new Map([
  ["screen_snapshot", "screenshot"],
  ["screen_move", "move"],
  ["screen_click", "click"],
  ["screen_double_click", "double_click"],
  ["screen_right_click", "right_click"],
  ["screen_type", "type"],
  ["screen_press", "press"],
  ["screen_scroll", "scroll"],
  ["screen_arm", "arm"],
  ["screen_disarm", "disarm"],
  ["screen_health", "health"],
]);

const TOOL_ARGUMENT_KEYS = new Map([
  ["screen_snapshot", new Set()],
  ["screen_move", new Set(["x", "y"])],
  ["screen_click", new Set(["x", "y"])],
  ["screen_double_click", new Set(["x", "y"])],
  ["screen_right_click", new Set(["x", "y"])],
  ["screen_type", new Set(["text"])],
  ["screen_press", new Set(["key"])],
  ["screen_scroll", new Set(["amount", "x", "y"])],
  ["screen_arm", new Set(["minutes"])],
  ["screen_disarm", new Set()],
  ["screen_health", new Set()],
]);

const boundedToolArguments = (name, value) => {
  const args = value === undefined ? {} : value;
  if (!args || typeof args !== "object" || Array.isArray(args)) {
    throw new Error("Arguments d'outil invalides");
  }
  const allowed = TOOL_ARGUMENT_KEYS.get(name);
  if (!allowed || Object.keys(args).some((key) => !allowed.has(key))) {
    throw new Error("Arguments d'outil invalides");
  }
  return Object.fromEntries(Object.entries(args).filter(([key]) => allowed.has(key)));
};

export const invokeWindowsScreen = (request, {
  bridge = process.env.CST_WINDOWS_AGENT_SCREEN_BRIDGE || "/usr/local/bin/cst-connect-windows-screen",
  spawnImpl = spawn,
} = {}) => new Promise((resolveRequest, rejectRequest) => {
  const child = spawnImpl(bridge, [], { stdio: ["pipe", "pipe", "ignore"] });
  let stdout = "";
  let settled = false;
  const finishError = (message) => {
    if (settled) return;
    settled = true;
    child.kill?.();
    rejectRequest(new Error(message));
  };
  const timer = setTimeout(() => finishError("Le poste Windows a mis trop de temps a repondre."), 55_000);
  child.once("error", () => finishError("Le pont securise vers l'ecran du PC est indisponible."));
  child.stdout.on("data", (chunk) => {
    if (settled) return;
    stdout += chunk.toString("utf8");
    if (Buffer.byteLength(stdout, "utf8") > MAX_LINE_BYTES) {
      finishError("La reponse de l'ecran est trop volumineuse.");
    }
  });
  child.once("close", () => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    const line = stdout.split(/\r?\n/, 1)[0];
    if (!line) {
      rejectRequest(new Error("L'ecran du PC n'a pas repondu."));
      return;
    }
    try {
      resolveRequest(JSON.parse(line));
    } catch {
      rejectRequest(new Error("L'ecran du PC a renvoye une reponse invalide."));
    }
  });
  child.stdin.end(`${JSON.stringify(request)}\n`);
});

const rpcError = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } });

// La capture est fournie en contenu image (base64 JPEG) pour les clients qui
// l'affichent, accompagnee d'un resume textuel avec l'echelle de conversion.
const resultContent = (result) => {
  const parts = [];
  const { screenshot: _screenshot, ...summary } = result;
  const text = JSON.stringify(summary, null, 2);
  if (result.screenshot) {
    const scale = result.screenWidth && result.width
      ? (result.screenWidth / result.width).toFixed(2)
      : null;
    const lines = [
      `Capture ecran : ${result.width}x${result.height} (ecran reel ${result.screenWidth}x${result.screenHeight}${scale ? `, echelle ${scale}` : ""}).`,
      `Pour cliquer sur un point vu dans l'image, multiplier ses coordonnees par ${scale || "l'echelle"} (coordonnees de l'ecran reel).`,
      "",
      text,
    ];
    parts.push({ type: "text", text: lines.join("\n") });
    parts.push({ type: "image", data: result.screenshot, mimeType: "image/jpeg" });
  } else {
    parts.push({ type: "text", text });
  }
  return { content: parts };
};

export const handleMcpRequest = async (request, {
  invoke = invokeWindowsScreen,
  screenSessionId = sessionId,
} = {}) => {
  if (!request || request.jsonrpc !== "2.0" || typeof request.method !== "string") {
    return rpcError(request?.id ?? null, -32600, "Requete JSON-RPC invalide");
  }
  if (request.method.startsWith("notifications/")) return null;
  if (request.method === "initialize") {
    return {
      jsonrpc: "2.0",
      id: request.id,
      result: {
        protocolVersion: typeof request.params?.protocolVersion === "string"
          ? request.params.protocolVersion
          : DEFAULT_PROTOCOL_VERSION,
        capabilities: { tools: { listChanged: false } },
        serverInfo: { name: "switch-pc-screen", version: "1.0.0" },
      },
    };
  }
  if (request.method === "ping") {
    return { jsonrpc: "2.0", id: request.id, result: {} };
  }
  if (request.method === "tools/list") {
    return { jsonrpc: "2.0", id: request.id, result: { tools: SCREEN_TOOLS } };
  }
  if (request.method !== "tools/call") {
    return rpcError(request.id, -32601, "Methode inconnue");
  }

  const name = request.params?.name;
  const action = TOOL_ACTIONS.get(name);
  if (!action) return rpcError(request.id, -32602, "Outil ecran inconnu");
  let args;
  try {
    args = boundedToolArguments(name, request.params?.arguments);
  } catch {
    return rpcError(request.id, -32602, "Arguments d'outil invalides");
  }
  try {
    const result = await invoke({
      ...args,
      kind: "agent-screen",
      sessionId: screenSessionId,
      action,
    });
    const isError = !result?.ok;
    return {
      jsonrpc: "2.0",
      id: request.id,
      result: {
        ...resultContent(result),
        ...(isError ? { isError: true } : {}),
      },
    };
  } catch (error) {
    return {
      jsonrpc: "2.0",
      id: request.id,
      result: {
        content: [{
          type: "text",
          text: String(error instanceof Error ? error.message : error).replace(/[\r\n]+/g, " ").slice(0, 1000),
        }],
        isError: true,
      },
    };
  }
};

export const runMcpServer = async () => {
  process.stdin.setEncoding("utf8");
  let buffer = "";
  for await (const chunk of process.stdin) {
    buffer += chunk;
    if (Buffer.byteLength(buffer, "utf8") > MAX_LINE_BYTES) {
      process.stderr.write("Requete MCP trop volumineuse.\n");
      process.exitCode = 1;
      return;
    }
    for (;;) {
      const newline = buffer.indexOf("\n");
      if (newline === -1) break;
      const line = buffer.slice(0, newline).trim();
      buffer = buffer.slice(newline + 1);
      if (!line) continue;
      let response;
      try {
        response = await handleMcpRequest(JSON.parse(line));
      } catch {
        response = rpcError(null, -32700, "JSON invalide");
      }
      if (response) process.stdout.write(`${JSON.stringify(response)}\n`);
    }
  }
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  runMcpServer().catch(() => { process.exitCode = 1; });
}
