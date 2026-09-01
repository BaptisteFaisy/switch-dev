import { createConnection } from "node:net";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import {
  AGENT_BROWSER_PIPE,
  MAX_AGENT_BROWSER_REQUEST_BYTES,
  MAX_AGENT_BROWSER_RESPONSE_BYTES,
  validateAgentBrowserRequest,
} from "./agent-browser-core.mjs";

const readStdin = async () => {
  const chunks = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    bytes += chunk.length;
    if (bytes > MAX_AGENT_BROWSER_REQUEST_BYTES) throw new Error("Demande navigateur trop volumineuse.");
    chunks.push(chunk);
  }
  return Buffer.concat(chunks).toString("utf8");
};

export const requestAgentBrowser = (payload) => new Promise((resolveRequest, rejectRequest) => {
  const socket = createConnection(AGENT_BROWSER_PIPE);
  let response = "";
  let settled = false;
  const fail = (message = "Le relais graphique Switch est indisponible.") => {
    if (settled) return;
    settled = true;
    socket.destroy();
    rejectRequest(new Error(message));
  };
  // Depasse le delai d'action du controller (90s) : le premier appel apres le
  // demarrage du broker peut inclure le lancement de Chrome + du proxy.
  socket.setTimeout(100_000, () => fail("Le navigateur Switch a mis trop de temps a repondre."));
  socket.once("error", () => fail());
  socket.on("data", (chunk) => {
    if (settled) return;
    response += chunk.toString("utf8");
    if (Buffer.byteLength(response, "utf8") > MAX_AGENT_BROWSER_RESPONSE_BYTES) {
      fail("Reponse navigateur trop volumineuse.");
      return;
    }
    const newline = response.indexOf("\n");
    if (newline === -1) return;
    settled = true;
    socket.end();
    try {
      resolveRequest(JSON.parse(response.slice(0, newline)));
    } catch {
      rejectRequest(new Error("Reponse invalide du navigateur Switch."));
    }
  });
  socket.once("connect", () => socket.write(`${payload}\n`));
});

export const runAgentBrowserClient = async () => {
  const raw = await readStdin();
  const value = validateAgentBrowserRequest(JSON.parse(raw));
  const result = await requestAgentBrowser(JSON.stringify(value));
  process.stdout.write(`${JSON.stringify(result)}\n`);
  if (!result?.ok) process.exitCode = 1;
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  runAgentBrowserClient().catch((error) => {
    process.stdout.write(`${JSON.stringify({
      ok: false,
      error: String(error instanceof Error ? error.message : error).replace(/[\r\n]+/g, " ").slice(0, 1000),
    })}\n`);
    process.exitCode = 1;
  });
}
