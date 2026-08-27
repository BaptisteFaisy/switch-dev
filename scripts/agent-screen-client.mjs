import { createConnection } from "node:net";
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

export const requestAgentScreen = (payload) => new Promise((resolveRequest, rejectRequest) => {
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
