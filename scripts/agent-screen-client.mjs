import { createConnection } from "node:net";
import { spawn } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import {
  AGENT_SCREEN_PIPE,
  MAX_AGENT_SCREEN_REQUEST_BYTES,
  MAX_AGENT_SCREEN_RESPONSE_BYTES,
  validateAgentScreenRequest,
} from "./agent-screen-core.mjs";

const readStdin = async () => {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > MAX_AGENT_SCREEN_REQUEST_BYTES) throw new Error("Demande ecran trop volumineuse.");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
};

const BROKER_SCRIPT = fileURLToPath(new URL("./agent-screen-broker.mjs", import.meta.url));

const probeBroker = () => new Promise((done) => {
  const socket = createConnection(AGENT_SCREEN_PIPE);
  socket.once("connect", () => { socket.destroy(); done(true); });
  socket.once("error", () => done(false));
});

// Le controle d'ecran doit etre utilisable par TOUS les chats, sans demarrage
// manuel : si le relais ne tourne pas, il est lance en arriere-plan (cache,
// sans console — rien a fermer ensuite) puis attendu. Une seconde instance
// s'arrete d'elle-meme si le pipe est deja pris (mono-instance par conception).
export const ensureBrokerRunning = async ({ spawnImpl = spawn, waitMs = 8000 } = {}) => {
  if (await probeBroker()) return;
  try {
    const child = spawnImpl(process.execPath, [BROKER_SCRIPT], {
      detached: true,
      stdio: "ignore",
      windowsHide: true,
    });
    child.unref?.();
  } catch {
    // La sonde ci-dessous arbitre : si le broker repond, on continue.
  }
  const deadline = Date.now() + waitMs;
  while (Date.now() < deadline) {
    await new Promise((resolveWait) => setTimeout(resolveWait, 250));
    if (await probeBroker()) return;
  }
  throw new Error("Le relais ecran Switch n'a pas pu demarrer automatiquement.");
};

const requestAgentScreenOnce = (payload) => new Promise((resolveRequest, rejectRequest) => {
  const socket = createConnection(AGENT_SCREEN_PIPE);
  let response = "";
  let settled = false;
  const fail = (message = "Le relais ecran Switch est indisponible.") => {
    if (settled) return;
    settled = true;
    socket.destroy();
    rejectRequest(new Error(message));
  };
  socket.setTimeout(52_000, () => fail("Le poste Windows a mis trop de temps a repondre."));
  socket.once("error", () => fail());
  socket.on("data", (chunk) => {
    if (settled) return;
    response += chunk.toString("utf8");
    if (Buffer.byteLength(response, "utf8") > MAX_AGENT_SCREEN_RESPONSE_BYTES) {
      fail("Reponse ecran trop volumineuse.");
      return;
    }
    const newline = response.indexOf("\n");
    if (newline === -1) return;
    settled = true;
    socket.end();
    try {
      resolveRequest(JSON.parse(response.slice(0, newline)));
    } catch {
      rejectRequest(new Error("Reponse invalide du poste Windows."));
    }
  });
  socket.once("connect", () => socket.write(`${payload}\n`));
});

// Demande publique : si le relais est absent (premier appel d'un chat, PC
// redemarre), le broker est demarre automatiquement puis la demande repartie
// une seule fois. Les depassements de temps ne sont jamais retentes.
export const requestAgentScreen = async (payload, { autoStart = true } = {}) => {
  try {
    return await requestAgentScreenOnce(payload);
  } catch (error) {
    const message = String(error instanceof Error ? error.message : error);
    if (!autoStart || !message.includes("indisponible")) throw error;
    await ensureBrokerRunning();
    return requestAgentScreenOnce(payload);
  }
};

export const runAgentScreenClient = async () => {
  const raw = await readStdin();
  const value = validateAgentScreenRequest(JSON.parse(raw));
  const result = await requestAgentScreen(JSON.stringify(value));
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (!result?.ok) process.exitCode = 1;
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  runAgentScreenClient().catch((error) => {
    process.stdout.write(`${JSON.stringify({
      ok: false,
      error: String(error instanceof Error ? error.message : error).replace(/[\r\n]+/g, " ").slice(0, 1000),
    })}\n`);
    process.exitCode = 1;
  });
}
