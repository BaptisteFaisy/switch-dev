import { createServer } from "node:net";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";
import { runAccountBrowserRequest } from "./open-account-browser.mjs";

const ACCOUNT_BROWSER_PIPE = "\\\\.\\pipe\\CodexSwitchAccountBrowser";
const MAX_REQUEST_BYTES = 65_536;

// Delai maximal d'une action : lancement a froid de Chrome + navigation proxy
// (60s) + marge sur un poste charge. Depasse par le timeout socket du client
// (130s) et par le delai socket du broker (+10s).
export const ACCOUNT_BROWSER_ACTION_TIMEOUT = 120_000;

const responseError = (error) => ({
  ok: false,
  error: String(
    error instanceof Error
      ? error.message
      : error || "La page Freebuff n'a pas pu etre ouverte dans le navigateur proxy."
  )
    .replace(/[\r\n]+/g, " ")
    .slice(0, 1000),
});

// Traitement d'une connexion : requete JSON sur une seule ligne, reponse sur
// une seule ligne, chien de garde par action. Isole du serveur pour pouvoir
// etre teste sans lier un vrai pipe nomme (hors contexte de logon, la liaison
// de pipe nommee peut echouer).
export const handleAccountBrowserConnection = ({
  socket,
  runner = runAccountBrowserRequest,
  requestTimeoutMilliseconds = ACCOUNT_BROWSER_ACTION_TIMEOUT,
}) => {
  let request = "";
  let handled = false;
  socket.setEncoding("utf8");
  // Chien de garde par action : sans lui, un lancement ou une page bloquee
  // laisse le chat attendre indefiniment — meme mecanique que les relais
  // ecran et navigateur agents. Le delai socket du broker depasse le delai
  // d'action pour laisser le temps de repondre l'erreur.
  socket.setTimeout(requestTimeoutMilliseconds + 10_000, () => socket.destroy());
  socket.on("data", (chunk) => {
    if (handled) return;
    request += chunk;
    if (request.length > MAX_REQUEST_BYTES) {
      handled = true;
      socket.end(`${JSON.stringify(responseError())}\n`);
      return;
    }
    const newline = request.indexOf("\n");
    if (newline === -1) return;
    handled = true;
    let value;
    try {
      value = JSON.parse(request.slice(0, newline));
    } catch {
      socket.end(`${JSON.stringify(responseError())}\n`);
      return;
    }
    let timer = null;
    const watchdog = new Promise((_resolve, rejectTimeout) => {
      timer = setTimeout(() => {
        rejectTimeout(new Error("Le poste Windows a mis trop de temps a ouvrir le navigateur proxy."));
      }, requestTimeoutMilliseconds);
    });
    timer.unref?.();
    Promise.race([runner(value), watchdog])
      .then((result) => socket.end(`${JSON.stringify(result)}\n`))
      .catch((error) => socket.end(`${JSON.stringify(responseError(error))}\n`))
      .finally(() => clearTimeout(timer));
  });
  socket.on("error", () => undefined);
};

export const createAccountBrowserBroker = ({
  pipeName = ACCOUNT_BROWSER_PIPE,
  runner = runAccountBrowserRequest,
  requestTimeoutMilliseconds = ACCOUNT_BROWSER_ACTION_TIMEOUT,
} = {}) => {
  const server = createServer((socket) =>
    handleAccountBrowserConnection({ socket, runner, requestTimeoutMilliseconds })
  );
  return server;
};

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  const server = createAccountBrowserBroker();
  server.on("error", () => process.exit(1));
  server.listen(ACCOUNT_BROWSER_PIPE);
}