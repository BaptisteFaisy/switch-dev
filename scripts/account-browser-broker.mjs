import { createServer } from "node:net";
import { runAccountBrowserRequest } from "./open-account-browser.mjs";

const ACCOUNT_BROWSER_PIPE = "\\\\.\\pipe\\CodexSwitchAccountBrowser";
const MAX_REQUEST_BYTES = 65_536;

const responseError = () => ({
  ok: false,
  error: "La page Freebuff n'a pas pu etre ouverte dans le navigateur proxy.",
});

const server = createServer((socket) => {
  let request = "";
  let handled = false;
  socket.setEncoding("utf8");
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
    void runAccountBrowserRequest(value)
      .then((result) => socket.end(`${JSON.stringify(result)}\n`))
      .catch(() => socket.end(`${JSON.stringify(responseError())}\n`));
  });
  socket.on("error", () => undefined);
});

server.on("error", () => process.exit(1));
server.listen(ACCOUNT_BROWSER_PIPE);
