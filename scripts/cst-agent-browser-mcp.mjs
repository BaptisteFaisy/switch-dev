import { createHash, randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const MAX_LINE_BYTES = 1_048_576;
const DEFAULT_PROTOCOL_VERSION = "2024-11-05";

export const browserSessionIdFromEnvironment = (
  environment = process.env,
  fallbackUuid = randomUUID,
) => {
  const seed = String(environment.CST_AGENT_BROWSER_SESSION_SEED || "").trim();
  if (seed && seed.length <= 4096) {
    const digest = createHash("sha256").update(seed, "utf8").digest("base64url");
    return `switch-${digest}`;
  }
  return `switch-${fallbackUuid()}`;
};

const sessionId = browserSessionIdFromEnvironment();

export const BROWSER_TOOLS = [
  {
    name: "browser_open",
    description: "Ouvre une page HTTP/HTTPS dans la fenetre Chrome Switch visible sur le PC. Utiliser uniquement lorsque l'utilisateur demande de consulter ou controler une page web. Le profil est dedie et separe de son Chrome personnel.",
    inputSchema: {
      type: "object",
      properties: { url: { type: "string", description: "URL HTTP ou HTTPS a ouvrir." } },
      required: ["url"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_snapshot",
    description: "Lit le texte visible et la liste des elements interactifs de la page Chrome Switch. Les references retournees ne restent valides que jusqu'au prochain instantane.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "browser_click",
    description: "Clique un element de la derniere vue de la page. Avant un achat, un envoi, une publication, une suppression ou un changement de compte, demander une confirmation explicite a l'utilisateur.",
    inputSchema: {
      type: "object",
      properties: { ref: { type: "string", description: "Reference opaque retournee par le dernier instantane." } },
      required: ["ref"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_fill",
    description: "Saisit du texte dans un champ de la page. Les mots de passe, codes OTP et donnees bancaires sont toujours refuses et doivent etre saisis manuellement par l'utilisateur dans Chrome.",
    inputSchema: {
      type: "object",
      properties: {
        ref: { type: "string", description: "Reference opaque du champ." },
        text: { type: "string", description: "Texte non sensible a saisir." },
      },
      required: ["ref", "text"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_select",
    description: "Choisit une option dans une liste deroulante de la page Chrome Switch.",
    inputSchema: {
      type: "object",
      properties: {
        ref: { type: "string", description: "Reference opaque de la liste." },
        value: { type: "string", description: "Libelle visible exact de l'option." },
      },
      required: ["ref", "value"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_press",
    description: "Appuie sur une touche de navigation sure dans la page Chrome Switch (Enter, Escape, Tab, fleches, etc.).",
    inputSchema: {
      type: "object",
      properties: {
        key: { type: "string", description: "Touche autorisee, par exemple Enter, Escape, Tab ou ArrowDown." },
        ref: { type: "string", description: "Reference optionnelle de l'element cible." },
      },
      required: ["key"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_back",
    description: "Revient a la page precedente dans l'onglet Chrome Switch controle.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "browser_close",
    description: "Ferme l'onglet Chrome Switch controle par cette conversation ou ce terminal.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
];

const TOOL_ACTIONS = new Map([
  ["browser_open", "open"],
  ["browser_snapshot", "snapshot"],
  ["browser_click", "click"],
  ["browser_fill", "fill"],
  ["browser_select", "select"],
  ["browser_press", "press"],
  ["browser_back", "back"],
  ["browser_close", "close"],
]);

const TOOL_ARGUMENT_KEYS = new Map([
  ["browser_open", new Set(["url"])],
  ["browser_snapshot", new Set()],
  ["browser_click", new Set(["ref"])],
  ["browser_fill", new Set(["ref", "text"])],
  ["browser_select", new Set(["ref", "value"])],
  ["browser_press", new Set(["key", "ref"])],
  ["browser_back", new Set()],
  ["browser_close", new Set()],
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

export const invokeWindowsBrowser = (request, {
  bridge = process.env.CST_WINDOWS_AGENT_BROWSER_BRIDGE || "/usr/local/bin/cst-connect-windows-browser",
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
  const timer = setTimeout(() => finishError("Le navigateur du PC a mis trop de temps a repondre."), 55_000);
  child.once("error", () => finishError("Le pont securise vers le navigateur du PC est indisponible."));
  child.stdout.on("data", (chunk) => {
    if (settled) return;
    stdout += chunk.toString("utf8");
    if (Buffer.byteLength(stdout, "utf8") > MAX_LINE_BYTES) {
      finishError("La reponse du navigateur est trop volumineuse.");
    }
  });
  child.once("close", () => {
    if (settled) return;
    settled = true;
    clearTimeout(timer);
    const line = stdout.split(/\r?\n/, 1)[0];
    if (!line) {
      rejectRequest(new Error("Le navigateur du PC n'a pas repondu."));
      return;
    }
    try {
      resolveRequest(JSON.parse(line));
    } catch {
      rejectRequest(new Error("Le navigateur du PC a renvoye une reponse invalide."));
    }
  });
  child.stdin.end(`${JSON.stringify(request)}\n`);
});

const rpcError = (id, code, message) => ({ jsonrpc: "2.0", id, error: { code, message } });

export const handleMcpRequest = async (request, {
  invoke = invokeWindowsBrowser,
  browserSessionId = sessionId,
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
        serverInfo: { name: "switch-pc-browser", version: "1.0.0" },
      },
    };
  }
  if (request.method === "ping") {
    return { jsonrpc: "2.0", id: request.id, result: {} };
  }
  if (request.method === "tools/list") {
    return { jsonrpc: "2.0", id: request.id, result: { tools: BROWSER_TOOLS } };
  }
  if (request.method !== "tools/call") {
    return rpcError(request.id, -32601, "Methode inconnue");
  }

  const name = request.params?.name;
  const action = TOOL_ACTIONS.get(name);
  if (!action) return rpcError(request.id, -32602, "Outil navigateur inconnu");
  let args;
  try {
    args = boundedToolArguments(name, request.params?.arguments);
  } catch {
    return rpcError(request.id, -32602, "Arguments d'outil invalides");
  }
  try {
    const result = await invoke({
      ...args,
      kind: "agent-browser",
      sessionId: browserSessionId,
      action,
    });
    const isError = !result?.ok;
    return {
      jsonrpc: "2.0",
      id: request.id,
      result: {
        content: [{ type: "text", text: JSON.stringify(result, null, 2) }],
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
