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
  "Coordonnee en pixels de l'ecran REEL si window est absent. Si window est fourni (id retourne par screen_windows), coordonnee CLIENT relative au coin haut-gauche de la fenetre cible, directement superposable a l'image de screen_snapshot.";

const windowProperty = {
  type: "integer",
  description: "Id de fenetre retourne par screen_windows : cible CETTE fenetre en arriere-plan, sans prendre le focus et sans toucher a la fenetre active. Omettez pour agir sur l'ecran global (fenetre active).",
};

export const SCREEN_TOOLS = [
  {
    name: "screen_snapshot",
    description:
      "Capture l'ecran complet du PC (tous les moniteurs) re-echantillonne en JPEG, ou une fenetre precise (window) par PrintWindow sans qu'elle soit au premier plan. Renvoie l'image, la resolution, l'echelle et la fenetre active. Utiliser en premier pour voir l'etat avant toute action.",
    inputSchema: {
      type: "object",
      properties: { window: windowProperty },
      additionalProperties: false,
    },
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
      "Clique gauche au point (x, y) de l'ecran reel ou d'une fenetre cible (window), puis renvoie une capture. Avec window, le clic est envoye par messages a CETTE fenetre sans prendre le focus. Une confirmation Windows locale s'affiche avant le clic (sauf session armee). Pour une action sensible (paiement, suppression, envoi), demander d'abord une confirmation explicite a l'utilisateur.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
        window: windowProperty,
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_double_click",
    description:
      "Double-clic gauche au point (x, y) de l'ecran reel ou d'une fenetre cible (window), puis renvoie une capture. Avec window, envoye sans prendre le focus.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
        window: windowProperty,
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_right_click",
    description:
      "Clic droit au point (x, y) de l'ecran reel ou d'une fenetre cible (window) pour ouvrir un menu contextuel, puis renvoie une capture. Avec window, envoye sans prendre le focus.",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "integer", description: coordinateDescription },
        y: { type: "integer", description: coordinateDescription },
        window: windowProperty,
      },
      required: ["x", "y"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_windows",
    description:
      "Liste les fenetres visibles du PC (id, pid, titre, position) pour choisir une cible de travail en arriere-plan. L'id retourne sert de parametre window aux autres outils : le chat pilote alors cette fenetre sans prendre le focus et sans voir le reste de l'ecran.",
    inputSchema: { type: "object", properties: {}, additionalProperties: false },
  },
  {
    name: "screen_locate",
    description:
      "Repere un texte a l'ecran (ou dans une fenetre cible via window) par OCR Windows et renvoie ses coordonnees de clic PRETES (clickX/clickY dans le meme espace que screen_click : ecran sans window, CLIENT avec window). Selectionnez un candidat avec index si plusieurs correspondent. Lecture seule, aucune confirmation.",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string", maxLength: 80, description: "Texte a repérer (insensible a la casse, sous-chaine)." },
        index: { type: "integer", minimum: 0, maximum: 50, description: "Index du candidat a selectionner si plusieurs lignes correspondent (0 = premiere)." },
        window: windowProperty,
      },
      required: ["text"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_open",
    description:
      "Lance une application en arriere-plan SANS prendre le focus (fenetre creee reduite et non activee). La commande est lancee directement par CreateProcess, sans intermediaire shell. Exemple : chrome \"https://www.google.com/search?q=meteo+Paris\". Utiliser ensuite screen_windows pour retrouver la fenetre cible et screen_snapshot pour la voir.",
    inputSchema: {
      type: "object",
      properties: {
        command: { type: "string", maxLength: 512, description: "Ligne de commande complete (executable + arguments), sans caracteres de controle." },
      },
      required: ["command"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_list",
    description:
      "Liste les onglets (pages) du navigateur lance en arriere-plan par screen_open (id de fenetre en parametre) : titre et URL de chaque onglet. Lecture seule, aucune confirmation.",
    inputSchema: {
      type: "object",
      properties: { window: windowProperty },
      required: ["window"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_screenshot",
    description:
      "Capture reelle de la PAGE (Chrome DevTools Protocol), independante du focus Windows : la fenetre peut rester en arriere-plan, reduite ou masquee. Utile pour voir le resultat d'une navigation ou l'etat d'un formulaire. Lecture seule, aucune confirmation.",
    inputSchema: {
      type: "object",
      properties: { window: windowProperty },
      required: ["window"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_navigate",
    description:
      "Navigue l'onglet actif du navigateur arriere-plan vers une URL (http/https), sans prendre le focus. Puis utiliser browser_screenshot pour voir la page. Action mutante : confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        window: windowProperty,
        url: { type: "string", maxLength: 4000, description: "URL complete (http:// ou https://)." },
      },
      required: ["window", "url"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_eval",
    description:
      "Execute une expression JavaScript dans la page (rendu JSON). Exemples utiles : document.title, document.body.innerText.slice(0, 4000), un selecteur de texte pour lire le contenu d'un article. Action mutante : confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        window: windowProperty,
        expression: { type: "string", maxLength: 4000, description: "Expression JavaScript a evaluer dans la page." },
      },
      required: ["window", "expression"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_click",
    description:
      "Clique sur l'element de la page correspondant au selecteur CSS (ex : a[href*='wikipedia'], button, input[type='submit']), en arriere-plan sans focus. Action mutante : confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        window: windowProperty,
        selector: { type: "string", maxLength: 500, description: "Selecteur CSS de l'element a cliquer." },
      },
      required: ["window", "selector"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_type",
    description:
      "Saisit un texte non sensible dans l'element de la page designe par le selecteur CSS (champ de formulaire), en arriere-plan sans focus. Les mots de passe, codes OTP et donnees bancaires sont refuses. Puis browser_key pour valider (Entree) si besoin. Action mutante : confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        window: windowProperty,
        selector: { type: "string", maxLength: 500, description: "Selecteur CSS du champ (input, textarea)." },
        text: { type: "string", maxLength: 2000, description: "Texte non sensible a saisir dans le champ." },
      },
      required: ["window", "selector", "text"],
      additionalProperties: false,
    },
  },
  {
    name: "browser_key",
    description:
      "Envoie une touche de navigation sure a la page (Enter, Escape, Tab, fleches, PageUp, PageDown, Home, End, F6, Ctrl+L...), en arriere-plan sans focus. Entree et Espace exigent une confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        window: windowProperty,
        key: { type: "string", description: "Touche a envoyer a la page (Enter, Escape, Tab, ArrowDown, ...)." },
      },
      required: ["window", "key"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_type",
    description:
      "Saisit un texte non sensible dans la fenetre active ou dans une fenetre cible (window, sans prendre le focus), puis renvoie une capture. mode \"messages\" (defaut) = WM_CHAR cible (apps Win32 classiques) ; mode \"focus\" = focus clavier temporaire par AttachThreadInput (ordre Z intact, marche partout y compris Chromium/WebView2) — le focus clavier de l'utilisateur est deplace quelques centaines de ms. Les mots de passe, codes OTP et donnees bancaires sont toujours refuses. Confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        text: { type: "string", description: "Texte non sensible a saisir (2000 caracteres max)." },
        window: windowProperty,
        mode: { type: "string", enum: ["messages", "focus"], description: "Strategie de saisie cible : messages (defaut) ou focus (universel)." },
      },
      required: ["text"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_press",
    description:
      "Appuie sur une touche de navigation sure (Enter, Escape, Tab, fleches, etc.) dans la fenetre active ou dans une fenetre cible (window, sans prendre le focus), puis renvoie une capture. mode \"focus\" = focus clavier temporaire par AttachThreadInput (ordre Z intact, marche partout). Entree et Espace exigent une confirmation Windows locale (sauf session armee).",
    inputSchema: {
      type: "object",
      properties: {
        key: { type: "string", enum: SAFE_KEY_ENUM, description: "Touche de navigation autorisee." },
        window: windowProperty,
        mode: { type: "string", enum: ["messages", "focus"], description: "Strategie cible : messages (defaut) ou focus (universel)." },
      },
      required: ["key"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_scroll",
    description:
      "Fait defiler la fenetre sous le curseur (molette) ou une fenetre cible (window, sans prendre le focus), puis renvoie une capture. amount positif = vers le haut, negatif = vers le bas (une cran = 1). Avec window, x/y optionnels en coordonnees clientes (defaut : centre).",
    inputSchema: {
      type: "object",
      properties: {
        amount: { type: "integer", description: "Nombre de crans de molette, entre -20 et 20, non nul." },
        x: { type: "integer", description: "Position X optionnelle du curseur avant defilement (ecran reel sans window, client avec window)." },
        y: { type: "integer", description: "Position Y optionnelle du curseur avant defilement." },
        window: windowProperty,
      },
      required: ["amount"],
      additionalProperties: false,
    },
  },
  {
    name: "screen_arm",
    description:
      "Arme la session de controle d'ecran : toutes les actions de cette session (clics, saisies, touches) passent SANS confirmation Windows jusqu'a expiration ou desarmement. AUCUNE popup n'est affichee : n'utiliser cet outil qu'apres une demande explicite et non ambigue de l'utilisateur (ex. « fais-le tout seul », « ouvre Chrome et cherche ») et pour une mission bornee dans le temps.",
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
  ["screen_windows", "windows"],
  ["screen_locate", "locate"],
  ["screen_open", "open"],
  ["browser_list", "browser"],
  ["browser_screenshot", "browser"],
  ["browser_navigate", "browser"],
  ["browser_eval", "browser"],
  ["browser_click", "browser"],
  ["browser_type", "browser"],
  ["browser_key", "browser"],
  ["screen_arm", "arm"],
  ["screen_disarm", "disarm"],
  ["screen_health", "health"],
]);

// Pour les outils navigateur, la methode CDP precise (list, navigate, eval,
// click, type, key, screenshot) est injectee dans la requete.
const TOOL_METHODS = new Map([
  ["browser_list", "list"],
  ["browser_screenshot", "screenshot"],
  ["browser_navigate", "navigate"],
  ["browser_eval", "eval"],
  ["browser_click", "click"],
  ["browser_type", "type"],
  ["browser_key", "key"],
]);

const TOOL_ARGUMENT_KEYS = new Map([
  ["screen_snapshot", new Set(["window"])],
  ["screen_move", new Set(["x", "y"])],
  ["screen_click", new Set(["x", "y", "window"])],
  ["screen_double_click", new Set(["x", "y", "window"])],
  ["screen_right_click", new Set(["x", "y", "window"])],
  ["screen_type", new Set(["text", "window", "mode"])],
  ["screen_press", new Set(["key", "window", "mode"])],
  ["screen_scroll", new Set(["amount", "x", "y", "window"])],
  ["screen_windows", new Set()],
  ["screen_locate", new Set(["text", "index", "window"])],
  ["screen_open", new Set(["command"])],
  ["browser_list", new Set(["window", "cdpPort"])],
  ["browser_screenshot", new Set(["window", "cdpPort"])],
  ["browser_navigate", new Set(["window", "cdpPort", "url"])],
  ["browser_eval", new Set(["window", "cdpPort", "expression"])],
  ["browser_click", new Set(["window", "cdpPort", "selector"])],
  ["browser_type", new Set(["window", "cdpPort", "selector", "text"])],
  ["browser_key", new Set(["window", "cdpPort", "key"])],
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
  if (result.ok && result.candidates && Array.isArray(result.candidates)) {
    // Resultat de screen_locate : liste lisible des candidats + candidat choisi.
    const space = result.space === "client"
      ? "CLIENT de la fenetre (utiliser avec screen_click window)"
      : "ECRAN reel (utiliser avec screen_click sans window)";
    const lines = [
      `Repérage « ${result.window ? "fenetre " + result.window + " : " : ""}${result.candidates.length} correspondance(s) — espace ${space} :`,
      ...result.candidates.slice(0, 12).map((c, i) =>
        `[${i}] (${c.clickX},${c.clickY}) « ${c.text.slice(0, 70)} »`),
      result.selected ? `Candidat selectionne (index ${result.index ?? 0}) : (${result.selected.clickX},${result.selected.clickY})` : "",
      "Passer clickX/clickY tels quels a screen_click (meme window). OCR : verifier la cible avec screen_snapshot avant une action sensible.",
    ].filter(Boolean);
    parts.push({ type: "text", text: lines.join("\n") });
    return { content: parts };
  }
  const text = JSON.stringify(summary, null, 2);
  if (result.screenshot) {
    if (result.pageCapture) {
      // Capture CDP de la page : independante du focus et de la visibilite.
      const lines = [
        `Capture de la page (arriere-plan, sans focus) : « ${result.title || ""} »`,
        `URL : ${result.url || ""}`,
        "Pour cliquer dans la page, utiliser browser_click avec un selecteur CSS plutot que des coordonnees (la mise en page peut differer de la capture).",
        "",
        text,
      ];
      parts.push({ type: "text", text: lines.join("\n") });
      parts.push({ type: "image", data: result.screenshot, mimeType: "image/jpeg" });
      return { content: parts };
    }
    const scale = result.screenWidth && result.width
      ? (result.screenWidth / result.width).toFixed(2)
      : null;
    const lines = result.window
      ? [
        `Capture fenetre ${result.window} : ${result.width}x${result.height} (fenetre reelle ${result.screenWidth}x${result.screenHeight}${scale ? `, echelle ${scale}` : ""}).`,
        `Les coordonnees de clic sont CLIENT (0,0 = coin haut-gauche de la fenetre) : multiplier une position vue dans l'image par ${scale || "l'echelle"}. La fenetre reste en arriere-plan, sans focus.`,
        "",
        text,
      ]
      : [
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
    const method = TOOL_METHODS.get(name);
    const result = await invoke({
      ...args,
      ...(method ? { method } : {}),
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
