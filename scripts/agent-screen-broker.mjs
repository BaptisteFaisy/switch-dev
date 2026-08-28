import { createServer } from "node:net";
import { appendFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import {
  AGENT_SCREEN_PIPE,
  AgentScreenController,
  MAX_AGENT_SCREEN_REQUEST_BYTES,
  MAX_AGENT_SCREEN_RESPONSE_BYTES,
} from "./agent-screen-core.mjs";

const publicError = (error) => ({
  ok: false,
  error: String(error instanceof Error ? error.message : error || "Le poste Windows a refuse la demande.")
    .replace(/[\r\n]+/g, " ")
    .slice(0, 1000),
});

export const createAgentScreenBroker = ({ controller = new AgentScreenController(), log = () => undefined } = {}) => {
  const server = createServer((socket) => {
    let request = "";
    let handled = false;
    socket.setEncoding("utf8");
    socket.setTimeout(52_000, () => socket.destroy());
    socket.on("data", (chunk) => {
      if (handled) return;
      request += chunk;
      if (Buffer.byteLength(request, "utf8") > MAX_AGENT_SCREEN_REQUEST_BYTES) {
        handled = true;
        socket.end(`${JSON.stringify(publicError("Demande ecran trop volumineuse."))}\n`);
        return;
      }
      const newline = request.indexOf("\n");
      if (newline === -1) return;
      handled = true;
      let value;
      try {
        value = JSON.parse(request.slice(0, newline));
      } catch {
        socket.end(`${JSON.stringify(publicError("Demande ecran illisible."))}\n`);
        return;
      }
      void controller.handle(value)
        .then((result) => {
          // Trace d'audit des changements d'etat de session armee.
          if (result && typeof result.armed === "boolean") {
            log(`session ${String(value?.sessionId || "?").slice(0, 40)} ${result.armed ? "armee" : "desarmee"}${result.armed ? ` (${result.minutes} min)` : ""}`);
          }
          const encoded = JSON.stringify(result);
          if (Buffer.byteLength(encoded, "utf8") > MAX_AGENT_SCREEN_RESPONSE_BYTES) {
            socket.end(`${JSON.stringify(publicError("La capture d'ecran est trop volumineuse pour etre transmise."))}\n`);
            return;
          }
          socket.end(`${encoded}\n`);
        })
        .catch((error) => socket.end(`${JSON.stringify(publicError(error))}\n`));
    });
    socket.on("error", () => undefined);
  });

  server.on("close", () => void controller.close());
  return { server, controller };
};

const brokerOptionsFromArguments = (args) => {
  const result = {};
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--log-file" && args[index + 1]) {
      result.logFile = args[index += 1];
    } else {
      throw new Error("Argument du broker ecran inconnu.");
    }
  }
  return result;
};

export const runAgentScreenBroker = ({ logFile } = {}) => {
  const log = (message) => {
    if (!logFile) return;
    try {
      appendFileSync(logFile, `${new Date().toISOString()} ${message}\n`, "utf8");
    } catch {
      // Le journal ne doit jamais rendre le broker indisponible.
    }
  };
  const controller = new AgentScreenController();
  const { server } = createAgentScreenBroker({ controller, log });
  const shutdown = () => {
    log("arret demande");
    server.close(() => void controller.close().finally(() => process.exit(0)));
    setTimeout(() => process.exit(1), 5000).unref();
  };
  process.once("SIGINT", shutdown);
  process.once("SIGTERM", shutdown);
  server.on("error", (error) => {
    log(`erreur serveur: ${String(error?.message || error)}`);
    process.exit(1);
  });
  server.listen(AGENT_SCREEN_PIPE, () => log("broker pret"));
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  try {
    runAgentScreenBroker(brokerOptionsFromArguments(process.argv.slice(2)));
  } catch (error) {
    process.stderr.write(`${String(error instanceof Error ? error.message : error)}\n`);
    process.exitCode = 1;
  }
}
